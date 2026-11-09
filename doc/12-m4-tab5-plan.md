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

### M4d 完了（2026-11-08）

- **kumiki-dtgen に P4 v1 の出力を追加した**（`kumiki-dtgen p4v1`）。生成器は `gen_esp.rs` にまとめ、`Backend::{Esp32s3, P4v1}` で次の点だけを切り替える。
  - 生成器自身が書く HAL 呼び出し: ピンのまとめ（`$p.GPIOn` / `$p.pins.gpioN`）、判別のプローブ、`init` の引数、`Delay`
  - バインディングのテンプレートの選び方: `kumiki.init-for.<backend>` があればそれを、なければ `init` を使う。embedded-hal と kumiki のドライバだけで書いたテンプレート（BMI270、GT911、touch-keys など）は両方の HAL で共通になる。
- `boards/dts/soc/espressif/esp32p4v1.dtsi`（手書き）と `boards/dts/m5stack/tab5.dts` を追加した。
  - GT911 のアドレス（INT の状態で 0x14 / 0x5D が決まる）は、second source の probe グループとして書いた。
  - IO エキスパンダの電源投入とパネルのロット判別は、glue ノード（`m5stack,tab5-display-power-sequence`）にした。中身は tab5-os の手順のままで、エキスパンダ #2（電源を切る線がある）には書き込まない。
  - ST7123 / ST7121 のロットは未対応。glue が `InitError::UnsupportedPanel` を返す。
- `kumiki-boards` に `chip-p4v1` を追加した。`p4v1/generated.rs` が生成物で、`Devices` と `Any*` の形は ESP32-S3 の家族と同じ。
- **同じアプリが両方の HAL で動く**: kumiki-demo の本体を `firmware/kumiki-demo/src/app.rs` に切り出し、ESP32-S3 用（`kumiki-demo`）と Tab5 用（`firmware-p4/kumiki-tab5-boards`）の両方で同じソースをビルドする。
  - 違いは入口の数行（HAL の初期化、`println!`、時刻と待ち）だけ。
  - Tab5 での結果: `detected M5Stack Tab5 (touch: GT911, imu: true, power: false)`。タッチの読み出しはエラー 0。

### その後（2026-11-08）

- **回転**: `FbDisplay` にソフトウェア回転を入れた。Tab5 は DT の `rotation = <270>` で横向き（1280×720）になる。タッチ座標は `AnyDisplay::touch_to_screen` で同じ向きに変換する。実機で向きとタッチの位置を確認した。
- **トリプルバッファ**: `DsiPanel` を 3 面にした。present で次の裏バッファにするのは 2 回前に表示を外れたバッファで、直近 2 回の変更領域を表示中のバッファから写す。
  - 結果: present は毎秒 33 回から 65 回（パネルのフレームレート）になった。present 1 回の最悪値は 15.3ms（垂直同期を待つ時間を含む）、アンダーランは 0。
- PPA の前のキャッシュ保守は行単位なので、幅 64px 未満の塗りつぶしとコピーは CPU で行う（tab5-os セッションの指摘）。

### HAL の整理（tab5-os セッションの提案、2026-11-08）

tab5-os（特権分離のあるマイクロカーネル）でも kumiki を使えるようにするための提案のうち、ベアメタルでも使いやすくなるものを入れた。

- **`rt` フィーチャ**（既定で有効）: `_start`、トラップベクタ、ESP-IDF のアプリ記述子、`entry!`、`main` の前のウォッチドッグ停止とリンカスクリプトをまとめた。OS のカーネルに組み込むときは外す（`time::init()` は自分で呼ぶ）。`--no-default-features` でも、riscv32imac（tab5-os のカーネル）と riscv32imafc の両方でビルドが通る。
- **コンソールの差し替え**: `println!` と HAL の `log!` の出力先は `console::set_sink(fn(&[u8]))` で決める。既定では USB Serial/JTAG（フィーチャ `usj-console`）に出し、フィーチャを外すと捨てる。
- **開始・確認・待ちの分割**: PPA にはもともと `start_*` / `poll` / `wait` がある。`Dsi::service()` は割り込みハンドラからも呼べることを明記した（状態を読んで消すだけで、割り込みの配線は呼び出し側が行う）。
- `wdt::disable_all()` と `time::init()` を公開した（`rt` がない場合に使う）。
- **残り**:
  - 状態を持たない低レベル層（ll）の切り出し
  - キャッシュ保守のトレイト化（OS が差し替えられるようにする）
  - チップリビジョン v3

  これらは tab5-os 側で実際に使う段階で行う。tab5-os 向けの実装（IPC 経由の I2C や FramebufferPanel）は tab5-os 側に置き、kumiki は tab5-os に依存しない。

### オーディオ（2026-11-09）

- **`kumiki-p4v1-hal` に I2S0 を追加した**（`i2s.rs`、`ahb_dma.rs`）。
  - 全二重、Philips 形式の 16 ビットステレオ、MCLK = 256·fs（40MHz の水晶から分数分周）。
  - 送受信とも、AHB-GDMA の 512 バイト × 32 個の循環リング（85ms 分）を使う。送信側は、`poll` が再生済みの区切りを無音で埋める。受信側は、区切りが 4 の倍数で終わらなくても 1 バイトも捨てない。
  - 受信は送信のクロックを `SIG_LOOPBACK` で共有する（IDF の全二重と同じ方式）。
- **Tab5 での確認**（`firmware-p4/kumiki-tab5-audio`）:
  - 送受信とも 48000 フレーム/秒。アンダーランもオーバーランも 0（例外は音の終わりに 1 回）。ES8388 と ES7210 は書き込みがすべて成功した。
  - 1kHz の音がスピーカーから鳴ることを利用者が確認した。マイクがその音を拾っているかは、レベルの変化だけでははっきりしなかった（同期検波での確認は未実施）。
  - 分かったこと: v1 の `RX_EOF_NUM` は `i2s_reg.h` の式どおりに動かず、n = 255 で 256 バイトごとに区切りが閉じた。害はないので、そのままにした。
  - 観測: `espflash monitor` / `reset` のあとに ROM のダウンロードモード（boot:0x204）で止まることがあった。ただし同じ時期に、tab5-os のツールが同じ Tab5 のデバッグプローブと USB シリアル（RTS によるリセットを含む）を操作していたと連絡があった。そのため、espflash 自体の問題かどうかは分からない。
- **ボード定義への組み込み**（ビルドまで確認、実機は未確認。Tab5 は tab5-os の作業に使うため）:
  - `esp32p4v1.dtsi` に i2s0、`tab5.dts` に ES8388（`amp-enable-gpios = <&ioexp1 1 …>`）と ES7210 を書いた。
  - 生成器: `espressif,esp32p4-i2s` のノードが I2S を作って開始し、`audio::SharedI2s` として共有する。`${outpin:PROP}` で、チップの GPIO か IO エキスパンダの線を `OutputPin` にする（PI4IOE5V6408 は、自分のビットだけを読み書きで変える `Pi4ioePin`）。
  - 安全策: `Pi4ioePin` は 0x44 の line 4（Tab5 の電源を切る線）を駆動しない。
- **実機で分かったこと（2026-11-09）: ES8388 は画面の電源投入より前に設定する。**
  - 画面の電源投入（IO エキスパンダ #1 の手順、LCD / タッチのリセットと GT911 の INT 同期）の後で ES8388 を設定すると、アンプの線が High でも、ES8388 のレジスタが正しく読み返せても、音が出なかった。
  - 診断用のビルド（`kumiki-tab5-audio-bisect`）で、ボード初期化の手順を 1 つずつ足しながらビープ音を鳴らして絞り込んだ。手順そのものはどれも原因ではなく、順序だけが原因だった。なぜ順序が効くのかは、まだ分かっていない。
  - 対策: ES8388 を先に設定し、画面の電源投入は出力レジスタの SPK_EN（ビット 1）を残すようにした。直した後、共通のデモでマイク（48007 フレーム/秒）と、スピーカーの初期化を確認した。

### OS のカーネル向けの整理、第 2 段（tab5-os の依頼 1〜5、2026-11-09）

- **割り込み**: `ahb_dma` に、チャネルごとの割り込みの有効化（`set_int_enable`）と状態の読み出し（`int_status`）、割り込み源の番号（rev v1 は IN_CHn = 56+n、OUT_CHn = 59+n。`interrupt_core0_reg.h` の hw_ver1 で確かめた。IDF の `interrupts.h` は v3 用で 1 つずれている）を足した。`I2s` には `interrupt_sources()` / `set_interrupts(bool)` / `on_interrupt()`（割り込みの要因を消して `poll` を呼ぶ）を足した。
- **DMA チャネルの選択**: `i2s::Config::dma_channel`（DT では `kumiki,dma-channel`、既定は 0）。
  - `ahb_dma::init()` はこれまでどおり、ブロックをリセットしてメモリの窓を L2MEM に設定する。
  - 自分で AHB-GDMA を設定する OS は、先に `ahb_dma::assume_initialized()` を呼ぶ。そうすると HAL はブロック全体に触らず、自分のチャネルだけを `configure` でリセットする。
  - 最初は「クロックが入っていれば使用中とみなしてリセットを飛ばす」形にしたが、Tab5 のスピーカーが鳴らなくなった。調べると、クロックはリセット直後から入っていて、窓は 0..0xFFFF_FFFF だった。そのため、明示的に呼ぶ形に直し、実機でスピーカーが鳴ることを確認した。
- **終わりのない待ちをなくした**:
  - SYSTIMER の値の確定待ち、値が安定するまでの再読み出し、クロックの分周の更新待ち（RAM で動くので回数で制限）、DSI の読み出し FIFO の掃き出し、`DsiPanel::present` のフレーム待ち（200ms でエラー）に上限を付けた。
  - 時間で待つループ（`delay_us`、`DelayNs`、`wait_us`）は、SYSTIMER が止まっていたら抜ける。
- **パニックをエラーに**: `AnyPin::steal`、`TxChannel::steal`、`RxChannel::steal` は、範囲外なら `None` を返す（`assert!` をやめた）。HAL に残る `assert!` は、コンパイル時のもの（アプリ記述子の大きさ）だけになった。
- **共有のクロックのレジスタ**: HP_SYS_CLKRST / LP_CLKRST / PMU の書き込みは、もともとすべて読んで一部だけ変える形だった（確認だけ）。
- ビルドは、`--no-default-features` を付けた場合と付けない場合のどちらも、riscv32imac / imafc で通る。Tab5 の共通デモで、画面・タッチ・マイク（48k/s）・スピーカーを確認した。

### コーデックの crate（2026-11-10）

- `kumiki-boards` に直書きしていた ES8388 / ES7210 の設定を、`drivers/kumiki-es8388` と `drivers/kumiki-es7210` に分けた。どちらも embedded-hal 1.0 の `I2c` だけを使い、ブロッキングで、待ち・alloc・panic はない。OS のカーネルからも使える（tab5-os が使う）。
  - ES8388: `init()`（M5Unified の 28 回の書き込み）、`set_volume(0..=100)`、`set_attenuation(0.5 dB 単位)`、`set_mute(bool)`。アンプ（SPK_EN）は扱わない。
  - ES7210: `init()`（リセットと 29 回の書き込み）、`set_gain(Mic, 0..=14)`。
  - 設定の値と順序は直書きの版と同じ。Tab5 でビープとマイクを確かめた。
- **Tab5 が SoC のリセットでは直らない無音の状態になることがある**:
  - tab5-os を動かした後、kumiki の版（前に鳴った 5263cf4 も含む）を書いても鳴らなかった。電源を完全に切って入れ直すと鳴った。
  - 鳴らないときと鳴るときで、ES8388 の 0〜52、エクスパンダ #1 の DIR / OUT / OUT_H_IM / PULL、I2S0 の 0x00〜0x3C、AHB-GDMA ch0 の OUT 側と窓、GPIO26〜30 の出力の選択と IO_MUX は、DMA の状態を除いてすべて同じだった。
  - 残っていたのは、レジスタからは見えない状態とみられる。原因は分かっていない。スピーカーが鳴らないときは、まず電源を入れ直して確かめる。
  - tab5-os が、電源を入れ直さずに戻す手順を見つけた。ES8388 の DAC の電源（レジスタ 4 = 0xC0）とチップの電源（レジスタ 2 = 0xFF）を 1 ms 落としてから init し直す。
    - init の始めのソフトリセット（レジスタ 0 = 0x80）では戻らない。SPK_EN を 1 秒落としても戻らない。
    - 起動直後（I2S を始めた直後）にやっても効かず、起動の後、最初に再生を始めるときにやると効いた。
  - そのため `Es8388::power_down()` を足した（`init` には入れていない）。kumiki の共通デモはまだこれを使っていない（Tab5 で試していないため）。

## 書き込みについて

- Tab5 に書き込むと、いま入っている tab5-os のカーネル（0x10000）が上書きされる。tab5-os の `make flash-p4` で戻せる。
- ブートローダ（0x2000）とパーティションテーブル（0x8000）は、tab5-os が IDF でビルドした v1 用のものをそのまま使う。
- 注意: 電源を落とす操作（PI4IOE5V6408 #2 の bit4）は、tab5-os の教訓どおり保護する（[02](02-local-assets.md)）。
