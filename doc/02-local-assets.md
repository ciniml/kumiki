# 02. このホストの既存資産（読み取りのみ）

すべて `~/repos` 配下にある。`target/` は対象外とした。

## 一覧

| リポジトリ | 環境 | 対象機種 | 現状 | 本プロジェクトにとっての価値 |
|---|---|---|---|---|
| stackchan-rs | no_std（esp-hal 1.1 + esp-rtos + embassy） | Core2, CoreS3 | 活発（2026-07〜08） | **最重要。** 自作ドライバ（m5drivers-rs）、DMA SPI、ボード切り替えの実例、落とし穴の記録 |
| tab5-os | no_std（esp-hal を使わず、レジスタを直接操作） | Tab5（ESP32-P4 rev v1.0） | 活発（2026-10〜11） | DSI・GDMA・PPA/DMA2D の実装と実測値 |
| m5paper-nameplate-rs（lgfx-rs） | std（esp-idf） | M5Paper | 停止（2023） | LovyanGFX を FFI で包む方式の限界を示す例 |
| m5stack-avatar-rs | no_std（HAL 非依存） | どれでも（DrawTarget） | 停止（2023）。stackchan-rs 側のコピーが後継 | アプリ側の利用例 |
| esp32-rust-examples | IDF + 独自ツールチェイン | M5Stack Basic | 歴史的資料（2020） | なし |

## stackchan-rs

### 構成

- `boards/m5stack/Cargo.toml` の依存: esp-hal 1.1（`unstable`）、esp-rtos 0.3（embassy）、embassy-executor 0.10、embassy-time 0.5、embedded-hal 1.0、embedded-hal-bus 0.3、embedded-graphics 0.8。
- **mipidsi と display-interface は使っていない。**

### ボードの切り替え

- feature の `core2` と `cores3` を排他にしている（`boards/m5stack/Cargo.toml:10-30`）。
- ターゲットトリプル（xtensa-esp32 と xtensa-esp32s3）はコマンドラインで別に指定する必要がある。
- 機種ごとの差分は `board/{core2,cores3}.rs` に置き、共通の型エイリアス（`Board`, `DisplayTy`, `TouchTy`, …）と cfg で吸収している。**トレイトは使っていない。**

### 表示まわり

- アプリ側の抽象は `AsyncDisplay` トレイト（`m5stack-avatar-rs/src/stackchan/mod.rs:523-531`）で、メソッドは `dimensions`、`async fill_rect`、`async blit(x,y,w,h,&[u8])` の 3 つ。
- CoreS3 のドライバは `m5drivers-rs/src/ili9342_dma.rs`。
  - `SpiBus` を占有し、CS はずっと Low のままにしている。
  - 初期化はブロッキングで行い、その後 `into_async` してから embedded-hal-async の `SpiBus::write` で送る。
  - 4KB のステージングバッファでバイトスワップしてから送る。
  - SPI2 を 40MHz で使い、バッファは `dma_buffers!(64, 16384)`（`board/cores3.rs`）。
- Core2 はブロッキング SPI で、1 行分のバッファを使う。実機では未検証。
- フレームバッファは持たない。ヒープ上の arena（64〜96KB）に描画グループごとのスプライトを作り、それぞれを blit する。重なるグループは結合してから送る（ちらつき対策。RECORD.md §12）。
- 性能
  - 描画タスクは 30fps で回している（`tasks/render.rs`）。
  - DMA 化する前は、コア 0 がブロッキング SPI で 1 フレームあたり約 20ms 塞がっていた（RECORD.md §11）。

### m5drivers-rs

- 実装済みのドライバ: AXP192, AXP2101, AW9523B, ILI9341, ILI9342（ブロッキング版と DMA 版）, FT6336, AW88298, PY32 IO エキスパンダ。
- **未実装のドライバ**: IMU（MPU6886, BMI270, BMM150）、RTC、ES7210, ES8311、ボタン。
- API の作り
  - I2C は embedded-hal 1.0 のブロッキング API。
  - I2C の共有は `embedded_hal_bus::i2c::RefCellDevice` で行う（シングルコア前提）。
  - エキスパンダのピンを `OutputPin` として実装している（`aw9523.rs:148-165`, `axp192.rs:133-150`）。これにより、LCD のリセットをエキスパンダ経由で渡せる。
  - `Axp2101::power_on_cores3()` は M5Unified のレジスタ設定手順をそのまま移植したもの（`axp2101.rs:100-125`）。
- ライセンス: MIT OR Apache-2.0。

### RECORD.md に記録された教訓（本プロジェクトでもそのまま当てはまる）

1. esp-hal-embassy は esp-hal 1.1 と組み合わせられないので、esp-rtos 0.3 を使う。
2. 古い xtensa GCC 8.4 のリンカは、エラーを出さずに壊れたバイナリを作る。espup で入る GCC 15.2 を使う。
3. スタックオーバーフロー
   - コア 0 のスタックは「DRAM の残り」になる。ヒープを大きくしすぎない。
   - opt-level=3 だと巨大な init が 1 つのスタックフレームにインライン展開される。`#[inline(never)]` と `StaticCell` で避ける。
4. 大きなバッファは Wi-Fi を起動する**前**に確保する（断片化対策）。`HEAP.free()` が十分でも、その大きさの連続領域があるとは限らない。
5. 循環 I2S DMA は `DmaError(Late)` から復帰できない。
6. Wi-Fi の動作中にフラッシュへ書き込むとクラッシュする。
7. 無印 ESP32 で esp-radio を使うと、ヒープは 112KB 程度までしか取れない。

## tab5-os（ESP32-P4 rev v1.0）

- 構成
  - 自作の M-mode RISC-V カーネル（安定版 Rust）で、アプリは U-mode の PIE。
  - esp-hal・PAC・embassy は使っていない。
  - プラットフォームの差は `kernel/src/platform/mod.rs` の `Platform` トレイトで分けている。
  - 専用エミュレータ（`emu/`）が DSI・GDMA・DMA2D をモデル化している。
- **esp-hal を使わない理由**: esp-hal が対応する P4 は rev v3.0 以降だけで、v1.x への対応は上流が受け入れない方針。PAC も v3 の SVD から生成されている（`doc/06-tab5-hardware.md:20-25`, `doc/03-rust-ecosystem.md:55-126`）。**手元の Tab5 実機の多くは v1.0 と考えられるので、esp-hal の `mipi_dsi` はそのままでは使えない**点に注意する。
- DSI の立ち上げ手順（`kernel/src/platform/esp32p4v1/display.rs:671-715`）
  1. D-PHY 用の LDO VO3 を 2.5V にする（eFuse の値で校正）。
  2. 2 レーン・1040Mbps に設定する（PLL は N=1, M=52, hsfreqrange 0x2A。M5GFX と同じ値）。
  3. DW-GDMA とブリッジを初期化する（DPI 80MHz、バーストモード）。
  4. ILI9881C の初期化表を DCS で送る（ID 0x98 0x81 を読み出して確認）。
  5. ビデオモードを開始し、バックライト（GPIO22）を点ける。
- パネルのロット違い
  - 初期のロットは ILI9881C + GT911（0x14）。
  - 後期のロットは ST7123 / ST7121（タッチ内蔵、0x55）。後期ロット向けは未実装。
- フレームバッファ: 720×1280 の RGB565（1.76MiB）を PSRAM の末尾 4MiB に置く。
- **GDMA の LLI リング**
  - 1 フレームを 1 個とする LLI を 16 個並べてリングにした。
  - IDF の方式（LLI 1 個を割り込みで毎回再起動する）では、カーネルが 0.5ms ほど塞がるだけでちらついていた。リングにしたことで約 230ms の猶予ができた（`display.rs:1-12,631-740`）。
- キャッシュ: CPU はキャッシュ経由で描画し、ROM の `Cache_WriteBack_Addr` で L1D → L2 の順に書き戻す（128B 単位）。
- **実測**: 約 64fps、アンダーランは 0、初期化から最初の画像表示まで 257ms（`doc/17-m6-display-design.md` §10）。
- PSRAM の帯域（`doc/43`）
  - スキャンアウトに約 118MB/s を使う。
  - L2 のラインを 128B にし、オートロードを有効にした結果は次のとおり。

  | 指標 | 変更前 | 変更後 |
  |---|---|---|
  | 書き込み | 60MB/s | 92MB/s |
  | 読み出し | 76MB/s | 190MB/s |
  | memcpy | 41MB/s | 75MB/s |

- PPA / DMA2D（`doc/44`, `ppa.rs`）
  - 全画面の塗りつぶし: **5.1ms（360MB/s）**。CPU だと約 20ms。
  - 半画面のコピー: 6.8ms。
  - A8 グリフの合成は CPU より速くならなかった。
  - 落とし穴
    - `BLEND_TRANS_MODE` の更新ビットは別の書き込みで立てる必要がある。
    - 重なりのあるコピーは、コピー先のアドレスが低い向きでしか安全に行えない。
- I2C の教訓: NACK の後にバスクリアをすると GT911 が応答しなくなる（`doc/17` §9）。
- Tab5 の IO エキスパンダ: PI4IOE5V6408 の 2 個目（0x44）は、bit4 に触ると電源が落ちる（`tab5.rs`）。
- ライセンス: クレートは MIT OR Apache-2.0。

## lgfx-rs（m5paper-nameplate-rs/lgfx-rs、stackchan-rs/boards/m5stack-idf/lgfx-rs）

- LovyanGFX を `LGFX_AUTODETECT` 付きでビルドし、C シム（`lgfx_c/lgfx_c.{h,cpp}`）と bindgen を経由して Rust から呼ぶ。IDF のコンポーネントとして組み込む。
- **std（esp-idf-sys）が前提**（README）。
- embedded-graphics アダプタは、1 ピクセルを `draw_line` 1 回で描いている。`fill_contiguous` は TODO のまま（`lgfx-rs/src/lib.rs:697-740`）。
- 上流は <https://github.com/ciniml/lgfx-rs>（最終 push 2024-02、std 前提）。
- **教訓**: FFI で包む方式は no_std・esp-hal・embassy の構成と相容れない。DrawTarget との境界でも性能を失う。今回の目的には合わない。

## まとめ（本プロジェクトへの示唆）

- 既存の no_std パターンは stackchan-rs の方式だけである（40MHz の DMA SpiBus、4KB のバイトスワップバッファ、RAM 上で合成してから blit）。これを一般化すればよい。
- 機種の差を吸収する共通トレイトはまだどこにもない。stackchan-rs の型エイリアスと cfg、エキスパンダのピンを `OutputPin` として渡す方式は、そのまま参考になる。
- Tab5 は、silicon の rev によっては esp-hal を使えない。tab5-os のレジスタ直叩き実装（DSI、GDMA リング、PPA）が最良の参考資料になる。
