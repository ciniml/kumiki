# 03. Rust のグラフィック関連エコシステム（2026-11-08 時点）

バージョンと日付は crates.io の API、リポジトリの活動状況は GitHub、API の詳細は各リポジトリのソースと CHANGELOG で確認した。esp-hal 1.2.2 は、ローカルの cargo レジストリにある公開版のソースも読んだ。「未確認」と書いた項目は一次情報で裏付けが取れていない。

## 1. embedded-graphics

- 0.8.2 / core 0.4.1（2026-02-15、MIT OR Apache-2.0）。この 2 つのリリースは MSRV と docs の修正だけで、その前のリリースは 0.8.1（2023-08）。<https://crates.io/crates/embedded-graphics>、<https://github.com/embedded-graphics/embedded-graphics/blob/master/CHANGELOG.md>
- master の Unreleased には破壊的変更が溜まっている。
  - `RawUx` との変換が必須になる（#765）。
  - `ByteOrder` が `DataOrder` に改名される（#767）。
  - `Rgb444` と `Rgb332` が追加される（#732, #768）。
  - `AlphaColor` と `ColorBlend` が追加される（#792）。
  - 0.9 のリリース時期は未確認。
- `DrawTarget`（<https://github.com/embedded-graphics/embedded-graphics/blob/master/core/src/draw_target/mod.rs>）
  - 必須のメソッドは `draw_iter` だけ。
  - それ以外は既定実装があり、`fill_contiguous(&Rectangle, IntoIterator<Item=C>)` → `draw_iter`、`fill_solid` → `fill_contiguous`、`clear` → `fill_solid` の順に委譲する。
- **性能上の制約**: すべてイテレータを経由する。スライスやバイト列を受け取る経路がないので、ドライバ側で 1 ピクセルずつ変換し、自分でまとめる必要がある。関連する issue は次のとおり（いずれも未解決）。
  - mipidsi #82: <https://github.com/almindor/mipidsi/issues/82>
  - mipidsi #142: <https://github.com/almindor/mipidsi/issues/142>
  - embedded-graphics #609: <https://github.com/embedded-graphics/embedded-graphics/issues/609>
- **非同期版の DrawTarget はない**。PR #747 は 2024-03 から未マージのまま。<https://github.com/embedded-graphics/embedded-graphics/pull/747>
- `ImageRaw<C, BO>` を `fill_contiguous` で描くと、`ImageDrawable::draw` からイテレータが渡される。ドライバは中身が連続したバッファかどうかを知ることができない。**本プロジェクトで拡張トレイトが必要になる根拠である。**
- フレームバッファ関連のクレート
  - 本体の `framebuffer::Framebuffer`（PR #673）
  - embedded-graphics-framebuf 0.5.0（2023-07）: <https://crates.io/crates/embedded-graphics-framebuf>
  - oxstack-gfx 0.2.1（2026-10、ワイヤ順の RGB565 と、書き換えた行の記録）: <https://crates.io/crates/oxstack-gfx>

## 2. mipidsi

- 0.10.0（2026-02-17、MIT）。<https://crates.io/crates/mipidsi>、<https://github.com/almindor/mipidsi>
- 対応パネル: GC9107, GC9A01, ILI9225, ILI9341, **ILI9342C**, ILI9486, ILI9488, RM67162, ST7735, **ST7789**, ST7796, NV3007（未リリース）。M5Stack の SPI パネルはほぼ揃っている。
- 0.9 で display-interface をやめ、独自の `interface::Interface` に移行した。理由は性能。<https://github.com/almindor/mipidsi/blob/master/docs/MIGRATION.md#v08---09>
  - 関連型 `Word`、定数 `KIND`、`send_command(u8,&[u8])`、`send_pixels<const N>(IntoIterator<Item=[Word;N]>)`、`send_repeated_pixel` を持つ。<https://github.com/almindor/mipidsi/blob/master/src/interface.rs>
  - `SpiInterface` は呼び出し側から受け取ったバッファにピクセルを詰め、ブロッキングの `SpiDevice::write` で送る。<https://github.com/almindor/mipidsi/blob/master/src/interface/spi.rs>
- `batch` feature（既定で有効）は、ピクセルを行や矩形にまとめる。
- **非同期対応はリリースされていない**（PR #179 などは未マージ）。**DMA を明示的に扱う仕組みもない**。下の `SpiDevice` が DMA を使っていれば、その分だけ速くなる。転送の完了を待たずに次の処理へ進む（重ねる）ことはできない。
- 評価: 初期化コマンド表、MADCTL、オフセットの扱いはそのまま使える品質。一方で、転送パイプラインと「送信開始後に待たない API」は構造的に入れにくい。

## 3. display-interface 系

- display-interface 0.5.0、-spi 0.5.0、-parallel-gpio 0.7.0（いずれも 2024-01）。保守のみの状態。
- 非同期版はあるが、DMA は組み込まれていない。統合 PR #48 は放置されている。<https://github.com/therealprof/display-interface/pull/48>
- mipidsi は display-interface から離れた。SSD1306 などは引き続き使っている。
- esp-display-interface-spi-dma 0.2.0（2024-12）は esp-hal 1.x より前のもので、現状では動かないと推定される。<https://github.com/georgik/esp-display-interface-spi-dma>

## 4. 非同期のディスプレイ関連

- **lcd-async** 0.1.3（2026-02、MIT）
  - mipidsi のフォークで、Interface を非同期にしたもの。
  - `RawFrameBuf`（ワイヤ順、DrawTarget を実装）に描いてから、`show_raw_data(...).await` で送る。
  - <https://github.com/okhsunrog/lcd-async>
- **esp-lcd-i8080** 0.1.0（2026-09）
  - esp-hal の `I8080` を使った mipidsi の Interface。**DMA ダブルバッファ**を持ち、最後のチャンクの完了を待たずに戻る。
  - LovyanGFX のバス層にいちばん近い既存実装である。
  - <https://github.com/zompinc/esp-lcd-i8080>
- embedded-hal-async 1.0.0。esp-hal は `SpiDma<'_, Async>` に対して `embedded_hal_async::spi::SpiBus` を実装している（esp-hal 1.2.2 `src/spi/master/dma.rs:218`）。

## 5. UI フレームワーク

- **Slint** 1.18.1（2026-09-21）
  - ライセンスは **GPL-3.0 / Royalty-free-2.0 / Commercial** の選択式。Royalty-free は組み込み用途を除外しているので、組み込み製品をクローズドで出すなら商用ライセンスが必要になる。<https://github.com/slint-ui/slint/blob/master/LICENSE.md>
  - no_std では `renderer-software` を使い、`render_by_line(LineBufferProvider)` で 1 行ずつ描画する。<https://docs.rs/slint/latest/slint/platform/software_renderer/index.html>
  - `mcu-board-support` に **m5stack_cores3** がある。mipidsi の `SpiInterface`（512B のバッファ）を使い、DMA は使っていない。<https://github.com/slint-ui/slint/blob/master/examples/mcu-board-support/m5stack_cores3/m5stack_cores3.rs>
  - 本プロジェクトでは、`LineBufferProvider` と矩形の blit をつなぐアダプタを用意すれば、DMA とパイプライン化で速くできる余地がある。
- **lvgl（lv_binding_rust）** 0.6.2（2023-04）は停止している。<https://github.com/lvgl/lv_binding_rust>
- **oxivgl** 0.9.2（2026-09、LVGL v9.5、nightly 必須）は活発に開発されている。<https://github.com/emobotics-dev/oxivgl>
- **mousefood** 0.5.3（ratatui を DrawTarget に描くバックエンド）: <https://github.com/ratatui/mousefood>
- **kolibri-embedded-gui**: <https://github.com/Yandrik/kolibri>

## 6. esp-hal

- 最新は **1.2.2（2026-09-18）**。1.0.0 は 2025-10-30 にリリースされた。<https://crates.io/crates/esp-hal>、<https://github.com/esp-rs/esp-hal/blob/main/esp-hal/CHANGELOG.md>
- **表示に関わる API はほぼすべて `unstable` 扱い**である（`dma`, `psram`, `i2s`, `lcd_cam`, `mipi_dsi`, `parl_io`。esp-hal 1.2.2 `src/lib.rs` の `unstable_module!`/`unstable_driver!`）。ライブラリから使う場合は `requires-unstable` を指定する。
- SPI と DMA
  - `Spi::with_dma(ch)` で `SpiDma` を作り、`.with_buffers(DmaRxBuf, DmaTxBuf)` でバッファを渡す。
  - **1.2.0 で `SpiDmaBus` は `SpiDma` に統合された**（#5272）。<https://github.com/esp-rs/esp-hal/pull/5272>
  - `write_buffer(DmaTxBuffer)` は転送ハンドルを返す。呼び出し側は転送中に別の処理を進め、あとで `wait()` してバッファを返してもらえる。**ダブルバッファにはこの API を使う。**
  - `DmaTxBuf` は `ExternalBurstConfig`（16/32/64B）で PSRAM からの転送にも対応する（`src/dma/buffers/mod.rs`）。
- ESP32-S3 の LCD_CAM
  - `lcd_cam::lcd::i8080::I8080::send(cmd, dummy, DmaTxBuffer) -> I8080Transfer`。8/16bit のバスに対応する。
  - `lcd_cam::lcd::dpi::Dpi` は RGB パネル用（`src/lcd_cam/lcd/{i8080,dpi}.rs`）。
- 無印 ESP32: `i2s::parallel::I2sParallel`（8/16bit、送信のみ）。
- **ESP32-P4**
  - 1.2.0 から対応した。**ただし silicon は rev v3.0 以降に限られる。**
  - `mipi_dsi`（`MipiDsi::new(MIPI_DSI, VDMA_CH0, Config)`、`dpi::DsiDpi::framebuffer_mut/commit/wait_for_vsync_async`）が入っている（#5596。esp-hal 1.2.2 `src/mipi_dsi/{mod,dpi}.rs`）。
  - **PPA と DMA2D は未対応**（README の対応表と、1.2.2 のソースを grep した結果）。PSRAM ドライバはまだ仮の実装。
- PSRAM: S3 は `PsramMode::Auto` で octal と quad を判別する（1.1.0, #5334）。PSRAM からの DMA は、キャッシュラインに揃えることが条件。
- C3 / C6: SPI master は stable。C6 の PARL_IO はパラレルバスとして使える可能性がある（未確認）。

## 7. M5GFX / LovyanGFX / M5Unified の Rust バインディング

- crates.io には `m5gfx`、`lovyangfx`、`lgfx` という名前のクレートはない。
- ciniml/lgfx-rs は std 前提で、休眠中（[02](02-local-assets.md) を参照）。
- **m5unified-rs**（`m5unified` 0.3.8、2026-05）は M5Unified を C シムで包んだもの。std（esp-idf）前提で、no_std では使えない。<https://github.com/mfiumara/m5unified-rs>
- **結論: no_std で M5GFX/LovyanGFX に相当するものは存在しない。**

## 8. M5Stack 向け no_std の小規模クレート（表示部分は未レビュー）

- m5stack-core 0.6.1（Fire27, CoreS3）: <https://crates.io/crates/m5stack-core>
- core-s3 0.5.6: <https://github.com/anapeksha/core-s3-rs>
- cardputer-adv 0.4.4: <https://crates.io/crates/cardputer-adv>
- oxstack-* 0.2.1（M5 系チップの非同期ドライバ群）: <https://github.com/theashraf/oxstack>
- epdpi-esp 0.1.0（LCD_CAM と DMA で駆動するパラレル EPD、M5PaperS3）: <https://crates.io/crates/epdpi-esp>

いずれも利用実績は小さい。M5Unified のように複数機種を網羅するものはない。

## 総括

| 機能 | 現状 | 本プロジェクトの方針 |
|---|---|---|
| 描画 API | embedded-graphics 0.8（ピクセル単位のイテレータ） | 互換を維持し、高速な経路を拡張トレイトで足す |
| パネルの初期化表 | mipidsi 0.10 で網羅されている | 参照するか、フィーチャーで取り込む（[05](05-build-vs-contribute.md)） |
| バスの抽象 | mipidsi の Interface（ブロッキング、待たずに進む API なし） | 新規に作る |
| SPI の DMA パイプライン | esp-hal の `write_buffer` はあるが、使っている高レベル実装がない | 新規に作る |
| i8080 / RGB | esp-hal にあり、esp-lcd-i8080 という先例もある | esp-hal の上に作る |
| DSI（P4） | esp-hal は v3 silicon のみ、PPA なし | esp-hal 版と tab5-os 流のレジスタ直叩き版の 2 本立て |
| スプライト / 変換 | 各クレートがばらばらに持っている | 新規に作る |
