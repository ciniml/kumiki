# 12. M4: Tab5（ESP32-P4 rev v1）の計画

調査日: 2026-11-08。手元の Tab5 は ESP32-P4 **rev v1.0**、フラッシュ 16MB（`espflash board-info`、tab5-os `doc/09-bringup-m1.md`）。

## 前提

- **esp-hal は使えない。** esp-hal は rev v3.0 以降だけを対象にしている。ロードマップ #3962 には「v1.3 chip support is not planned and contributions to add it will not be accepted」と書かれている。esp-hal でビルドしたアプリが v1.3 の Tab5 でブートローダから飛んだ直後に止まる報告もある（#6179）。出典は tab5-os `doc/03-rust-ecosystem.md` §1。
- **tab5-os に実機で動く実装がある**（同じ作者、MIT OR Apache-2.0）。
  - ESP-IDF の `hw_ver1` レジスタ定義を一次情報にして、必要なブロックだけを手書きしている。
  - IDF の v1 用 2nd stage ブートローダから起動し、0x10000 から XIP で実行する。
  - DSI は GDMA の LLI リングで 64fps を出す。
  - PPA で全画面の塗りつぶしが 5.1ms。
  - PSRAM（HEX、200MHz）の初期化も自前。
  - 参照するファイル: `kernel/src/platform/esp32p4v1/{clock,psram,i2c,display,ppa,tab5}.rs`、`kernel/kernel-common.ld`、`boot/`
- kumiki のボード定義（DTS → 生成）は HAL アダプタを挟む設計なので（[07](07-design-hwdesc.md) §9）、P4 v1 用の自前 HAL を差し込める。**Tab5 は「esp-hal 以外の HAL でも kumiki-dt が成り立つか」を確かめる最初の実例になる。**

## 構成

```
hal/kumiki-p4v1-hal      ESP32-P4 rev v1 用の最小 HAL（no_std）
  rt        エントリ、リンカスクリプト（IDF ブートローダ用の 2 セグメント配置）、トラップの最小処理
  clock     CPU / PLL / ペリフェラルのクロック
  psram     HEX PSRAM の初期化とキャッシュ設定（L2 ライン 128B、自動ロード）
  gpio      IO_MUX / GPIO マトリクス、embedded-hal の OutputPin / InputPin
  i2c       I2C マスタ（embedded-hal の I2c、NACK のあとにバスクリアをしない）
  usb_jtag  USB Serial/JTAG への出力（ログ）
  delay     SYSTIMER による DelayNs
  dsi       MIPI-DSI ホスト + DPI ブリッジ + D-PHY PLL + LDO VO3
  gdma      DW-GDMA の LLI リング（フレームバッファの走査、16 本のリング）
  ppa       PPA の塗りつぶしと DMA2D のコピー
  cache     L1 / L2 の書き戻し（ROM 関数）
gfx/kumiki-gfx           FramebufferPanel（新設）: back_buffer / flush_cache / present / accel
gfx/kumiki-gfx-panels    ILI9881C / ST7123（DSI の初期化表）
boards/dts/m5stack/tab5.dts, boards/dts/soc/espressif/esp32p4v1.dtsi
dt/kumiki-dtgen          p4v1 バックエンド（HAL アダプタ違い）
```

## 段階

| 段階 | 内容 | 完了の条件 |
|---|---|---|
| M4a | `kumiki-p4v1-hal` の骨格: rt、USB Serial/JTAG、SYSTIMER、GPIO、I2C | Tab5 で起動ログが出て、内部 I2C をスキャンできる（PI4IOE5V6408 0x43/0x44、GT911 0x14 など） |
| M4b | PSRAM + DSI + GDMA リング + ILI9881C。`FramebufferPanel` を kumiki-gfx に追加 | Tab5 の画面に kumiki-gfx で描いた画像が出る。present が垂直同期に同期する（約 60fps） |
| M4c | PPA を `FramebufferPanel::accel` として使い、塗りつぶしとコピーを `FastDraw` から使えるようにする | 全画面の塗りつぶしが 6ms 以下（tab5-os の 5.1ms が目安） |
| M4d | Tab5 のボード定義: PI4IOE5V6408、GT911 / ST7123 のタッチ、BMI270、パネルのロット違いの判別。DTS と p4v1 用の生成バックエンド | 同じアプリ（kumiki-demo 相当）が Tab5 で動き、ボード定義が DTS から生成される |

## 進捗

### M4a 完了（2026-11-08）

- `hal/kumiki-p4v1-hal`（rt、USB Serial/JTAG、SYSTIMER、GPIO、I2C、ウォッチドッグの停止）と `firmware-p4/kumiki-tab5-hello` を追加した。
  - ターゲットは `riscv32imafc-unknown-none-elf`（stable のツールチェイン）。`_start` で FPU を有効にする（mstatus.FS）。
  - I2C は embedded-hal の `I2c` を実装した。長い書き込みと読み出しは、END コマンドで複数のコマンドリストに分ける。
- Tab5 の実機での結果（ログ全文は省略）:
  - IDF の v1 用ブートローダから起動した。フラッシュのセグメントは 2 つ（appdesc + rodata、text）。
  - misa `0x40901125`、MAC `30:ed:a0:e0:cb:86`（tab5-os の記録と一致）。
  - 内部 I2C（GPIO31/32、100kHz）のスキャン結果: 0x10（ES8388）、0x14（GT911）、0x32（RX8130）、0x36、0x40（ES7210）、0x41（INA226）、0x43 / 0x44（PI4IOE5V6408）、0x68（BMI270）。
  - PI4IOE5V6408 #1 のレジスタ 0x01 は `0xa0`。
- **CPU クロックは約 89MHz**（`mcycle` を SYSTIMER と比べて測定）。ブートローダは CPU を最高速度（360MHz）にしていない。M4b の前に、tab5-os の `clock.rs` を移植して CPU と PLL を設定する。

### M4b / M4c 完了（2026-11-08）

- tab5-os から `clock`（PVT と 360MHz）、`cache`、`psram`（HEX 200MHz、32MiB、タイミング調整）、`dsi`（ILI9881C、GDMA の LLI リング）、`ppa` を移植した。割り込みは使わず、`Dsi::service()` をポーリングで呼ぶ。
- `kumiki-tab5-demo`（フレームバッファへの直接描画）: 64fps、アンダーラン 0。PPA の全画面塗りつぶしは 5.16ms（tab5-os の 5.1ms と同じ）、全画面コピーは 9.4ms。色の並びと動く箱を目視で確認した。
- kumiki-gfx に `FramebufferPanel` / `FbDisplay` を追加し、HAL の `fb::DsiPanel`（feature `gfx`）で DSI を `FramebufferPanel` にした。
  - ダブルバッファ。`present` は表示を切り替え、2 フレーム待ってから変更領域を PPA で新しい裏バッファへコピーする。
  - 4096 ピクセル以上の塗りつぶしとコピーは PPA で行う。PPA が書く領域は、先にキャッシュを書き戻して無効化する。
- `kumiki-tab5-gfx`（embedded-graphics の文字・図形 + `FbDisplay`）の実測値:

| 項目 | 結果 |
|---|---|
| 全画面のクリア（PPA） | 5.2ms |
| 画面全体の描画（文字・矩形・円・三角形） | 10.7ms |
| 最初の present（全画面の書き戻しとコピー） | 23.6ms |
| 動く箱の差分更新 | 33 回/秒（present ごとに 2 フレーム待つため）、アンダーラン 0 |

- 残り: present の待ちを減らす（切り替えの完了を 1 フレームで判定できるか、裏バッファを 3 枚にするか）。ST7123 パネルは未対応（tab5-os でも未対応、手元の個体は ILI9881C）。

## 書き込みについて

- Tab5 に書き込むと、いま入っている tab5-os のカーネル（0x10000）が上書きされる。tab5-os の `make flash-p4` で戻せる。
- ブートローダ（0x2000）とパーティションテーブル（0x8000）は、tab5-os が IDF でビルドした v1 用のものをそのまま使う。
- 注意: 電源を落とす操作（PI4IOE5V6408 #2 の bit4）は、tab5-os の教訓どおり保護する（[02](02-local-assets.md)）。
