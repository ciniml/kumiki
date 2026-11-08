# 11. マイルストーンと計画

## 決定事項（2026-11-08）

| 項目 | 決定 |
|---|---|
| ハードウェア記述の構文 | **Device Tree Source（DTS）**（[07](07-design-hwdesc.md)） |
| クレート名 | **M5 を付けない**。プロジェクト名と接頭辞は **`kumiki`**（DT の vendor prefix も `kumiki,`。[09](09-crate-layout-license.md)） |
| 非同期 | **async を既定にする。非同期ランタイムは embassy に固定しない**（[07](07-design-hwdesc.md) §7） |
| Tab5 | 手元の機体は **ESP32-P4 rev v1**。esp-hal は使えないので、tab5-os 由来の自前 HAL で対応する（[07](07-design-hwdesc.md) §9） |
| ライセンス | **MIT OR Apache-2.0**（Rust の標準） |
| 外部との連携 | **行わない**。まず利用者がよく使うボードの定義から書く（[08](08-design-bsp.md) §7） |
| ボードの順序 | **CoreS3 / AtomS3 → Tab5**。**無印 ESP32 系は対象外** |

## 進め方の方針

- **コード生成器は、手書きのコードを先に作ってから作る。** M2 で「生成されるはずの形」のコードを CoreS3 と AtomS3 向けに手で書き、M3 でそれを DTS から再現する生成器を作る（生成結果をゴールデンファイルとして比較する）。生成器を先に作ると、実際のドライバの組み合わせで困る点（所有権、共有バス、ピンの型）を後から知ることになり、作り直しが増えるためである。
- 各マイルストーンの終わりに、ベンチマークの結果を `doc/bench-results/` に残す。

## マイルストーン

### M0: 土台と基準値

- cargo のワークスペース、CI を用意する。
  - ビルドの対象: esp32s3（xtensa）、riscv32imafc（P4）、thumbv6m（RP2040、汎用性の確認用）
  - CI で回すもの: fmt、clippy、ホスト上のテスト
- ツールチェインの手順を文書化する（espup、GCC 15.2 系のリンカ）。
- **M5GFX のベンチマークのスケッチ**（B1〜B7、[10](10-performance-plan.md)）を作り、CoreS3 と AtomS3 で基準値を取る。
- 完了の条件: M5GFX の基準値の表ができていること。

### M1: グラフィックの中核

- `kumiki-gfx`: ピクセル形式、`PixelSource`、Bus/Panel のトレイト、`Display`、`Sprite`（等倍コピーと透過）、`FastDraw`
- `kumiki-gfx-panels`: `DcsPanel` + ILI9342C、GC9107
- `kumiki-gfx-bus-esp`: `EspSpiDmaBus`（`write_buffer` を使った A/B のパイプライン、`write_repeat`）
- `kumiki-gfx-bus-ehal`: `SpiDeviceBus` と `AsyncSpiBus`
- B1〜B4 と B8 を測る。
- 完了の条件: CoreS3 で B2 が M5GFX の 80% 以上。

### M2: 最初のマイルストーン（CoreS3 と AtomS3 で同じアプリが動く）

- `kumiki-class`: Display、Touch、Power、Buttons、Imu の最小限のトレイト（ブロッキング版と async 版）
- `kumiki-boards`（`chip-esp32s3`）を、生成されるはずの形で**手書き**する。
  - ボードの判別: AXP2101 と AW9523 があれば CoreS3、なければ GC9107 の RDDID を読んで AtomS3
  - CoreS3: AXP2101 と AW9523 による電源投入、GPIO35 の切り替え、FT6336U、BMI270
  - AtomS3: PWM のバックライト、GPIO41 のボタン、MPU6886（ドライバを新しく書く）
  - ボタンは共通のビットマスクで扱う（CoreS3 は電源キーとタッチ領域、AtomS3 は GPIO）
  - バックグラウンドの処理は、ランタイムに依存しない Future（`Devices::tasks`）として返す。
- `examples/same-app`: **1 つのバイナリ**が CoreS3 と AtomS3 の両方で起動する。画面の大きさに合わせて描画内容を変え、ボタンかタッチで画面を切り替え、IMU の値を表示する。
- **完了の条件**
  1. 同じバイナリ（`chip-esp32s3` で、ボードの feature は指定しない）が両方のボードで動く。
  2. 全画面の DMA 転送（B2）が、両方のボードで M5GFX の **80% 以上**の fps を出す。
  3. B1、B3、B4 も 80% 以上を出す。B5、B9 は測定して記録する。
  4. 同じアプリが **2 種類の実行環境**で動く: embassy-executor と、embassy を使わない最小の executor（`block_on` と `join` で回す簡易なもの）。さらに、ブロッキング版の見本もある。

### M3: kumiki-dt v0（DTS からの生成）

- `kumiki-dt`: 前処理器（#include、#define、#if）、DTS v1 の構文解析、`/delete-node/`、ラベル、オーバーレイ、合成、検証
- `boards/bindings/*.yaml`（Zephyr の書式に `kumiki:` セクションを足したもの）と `kumiki-bindings`
- `kumiki-dtgen`: 依存グラフ、初期化順序、`Devices`、`init()`、判別の決定木を生成する。CLI は `check / gen / pintable / dump`。
- esp-metadata から `esp32s3.dtsi` を生成する。P4 v1 の `.dtsi` は手で書く。
- **M2 の手書きのコードを、CoreS3 と AtomS3 の DTS から再現する**（ゴールデンファイルで比較する）。
- CI で、すべての DTS が標準の `dtc` でもコンパイルできることを確かめる。
- DTS を書くだけで AtomS3R に対応できることを示す。ここで手書きのコードは書かない（kumiki-dt の価値を確かめるため）。
- 汎用性の確認: ESP32-S3-DevKitC + ST7789、RP2040 + ST7789（`kumiki-hal-rp`）。

### M4: Tab5（rev v1）

- `kumiki-p4v1-hal`
  - まず `esp-hal-p4-pre-v3` を rev v1.0 の機体で短期間評価する。使えなければ、または足りなければ、tab5-os の実装（I2C、GPIO、PSRAM、クロック、DSI、GDMA の LLI リング、PPA/DMA2D）を embedded-hal を実装した HAL として切り出す。
- `kumiki-gfx` の `FramebufferPanel` を DSI で実装する（ダブルバッファ、キャッシュの書き戻し、vsync）。
- PPA を `FramebufferPanel::accel` として使う（塗りつぶし、コピー）。
- DSI パネルのロット違い（ILI9881C、ST7123、ST7121）を、DT の second source と DSI の ID 読み出しで判別する。GT911 と ST7123 のタッチ。
- `tab5.dts` を書き、kumiki-dt で生成する（esp-hal 以外の HAL で kumiki-dt が成り立つことの確認を兼ねる）。

### M5: 機能の拡充

- スプライトの回転・拡縮、パレット、`DirtyRegions`、ダブルバッファ
- RTC、オーディオ（CoreS3 の AW88298 / ES7210、Tab5 の ES8388 / ES7210）
- Slint のアダプタ（任意）
- 必要に応じて ESP32-S3 系のボードを追加する（Cardputer、PaperS3 など）

## 主なリスク

| リスク | 対策 |
|---|---|
| esp-hal の unstable API が変わる | 依存を kumiki-gfx-bus-esp と kumiki-hal-esp に閉じ込める。版は範囲で指定し、CI の行列テストで確かめる |
| esp-hal の `write_buffer` の固定費が大きく、M5GFX の 80% に届かない | 小さな転送は FIFO へ直接書く経路を持つ。最終手段として、SPI のレジスタを直接叩く経路を unsafe で用意する（M5GFX と同じ方式） |
| DTS のパーサと前処理器の作り込みが膨らむ | 対象を DTS v1 と、ボードの記述に必要な前処理だけに絞る。標準の `dtc` との互換は CI で担保する |
| P4 rev v1 用の HAL の保守 | 対象を Tab5 が使うペリフェラルに限る。tab5-os のエミュレータとテストを流用する |
| 生成器のスキーマ（バインディングの書式）が早い段階で固まりすぎる | M3 では v0 として破壊的変更を許す。M4 と M5 でボードを増やしてから v1 とする |

## 進捗（2026-11-08）

| マイルストーン | 状況 |
|---|---|
| M0 | 完了：ワークスペース、M5GFX 比較ベンチ（`bench/m5gfx-idf`）、基準値 |
| M1 | 完了：全画面転送で M5GFX の 99%（CoreS3 / AtomS3。[bench-results](bench-results/)） |
| M2 | 完了：同じバイナリで CoreS3 / AtomS3 / StopWatch を自動判別。async 版（自作 `block_on` と embassy の両方）を AtomS3（113.7fps）と CoreS3（25.8fps、全画面を毎フレーム描画＋転送）で確認。BMI270 は CoreS3 と StopWatch の両方で動作 |
| 追加 | StopWatch（QSPI の CO5300、`BufferedDisplay`、M5PM1 / M5IOE1、CST820 / CST9217、BMI270）を前倒しで対応。表示・ボタン・IMU・タッチを実機で確認 |
| M3 | v0 完了：`kumiki-dt`（前処理・構文解析・合成・phandle 解決）、3 機種の DTS（`dtc` でも警告なし）、YAML バインディング、`kumiki-dtgen`（検証・初期化順序・コード生成）。`kumiki-boards` は生成された判別関数・ピンのまとめ・定数を使う形に移行し、StopWatch で動作を確認。残り（v1）：初期化の手順そのものをバインディングのテンプレートから生成する、esp-metadata から SoC の `.dtsi` を生成する |
| M3 v1 | 初期化の生成：バインディングの `kumiki.init` / `probe` / `gpio-pin` テンプレートを INIT_ORDER の順に展開し、機種ごとの `init` と `init_board` を生成。手書きの `init_*` を廃止。AXP2101 はレギュレータノード、AW9523 は `gpio-hog`、LCD リセットは `reset-gpios`（エキスパンダのピン）から汎用に設定。StopWatch の非公開レジスタ手順は glue ノード（`m5stack,stopwatch-power-sequence`）。second source は probe グループとして生成。3 機種すべてで動作確認済み（CoreS3 は汎用化した AXP2101 / AW9523 / エキスパンダ経由の LCD リセットで起動）。残り：esp-metadata からの `.dtsi` 生成、`Any*` の生成 |
| M3 v1 追加 | `Any*` の生成：バインディングの `kumiki.variant` / `kumiki.type`（`type-for` で HAL ごと）から、家族ごとに `AnyDisplay` / `AnyTouch` / `AnyImu` / `AnyPower` / `AnyBacklight` と、ディスパッチ用のマクロ（`any_touch!(t, v => expr)` など）を生成する。トレイトの実装は分類ごとに 1 回だけ手で書くので、DTS とバインディングにデバイスを足すだけで手書きの変更は要らない。ドライバ側は kumiki-class のトレイト（`Imu` など）を自分で実装し、エラーは `From` で `AnyError` にまとめる |
| M3 完了 | SoC の `.dtsi` の生成：`kumiki-dtgen soc-dtsi esp32s3 <out>` が、firmware の Cargo.lock に固定された esp-metadata-generated（GPIO、I2C / SPI マスタ、Octal PSRAM が使うピン）と PAC（レジスタの番地）から `esp32s3.dtsi` を作る。値は手書きのものと同じで、ボードの生成物は変わらない。P4 rev v1 は esp-metadata に定義がないため手書きのまま |

| M4 | a〜d 完了（詳細は [12](12-m4-tab5-plan.md)）：ESP32-P4 rev v1 用の自前 HAL（`kumiki-p4v1-hal`）、DSI 表示 64fps、PPA の全画面塗りつぶし 5.2ms、`FramebufferPanel` / `FbDisplay`。`tab5.dts` から `kumiki-dtgen p4v1` でボード定義を生成し、ESP32-S3 用と同じデモのソースが Tab5 でも動く（esp-hal 以外の HAL でも kumiki-dt が成り立つことを確認）。残り：ST7123 のパネル、画面の回転、present の待ちの短縮 |

| 共通化の確認 | アプリ本体を HAL から独立させ（`firmware/kumiki-demo/src/app.rs`）、`Any*` を生成に切り替えた後のバイナリで、CoreS3 / AtomS3 / StopWatch / Tab5 の 4 機種の判別と初期化を確認した（2026-11-08） |

| M5（進行中） | スプライトの回転・拡大縮小（`Sprite::push_rotate_zoom`、M5GFX の `pushRotateZoom` 相当）。RTC（`kumiki-class::Rtc`、`kumiki-rtc` に PCF8563 / BM8563 と RX8130、DT の `chosen { kumiki,rtc }` から生成）。Tab5 の RX8130 と CoreS3 の BM8563 の両方で、読み出しと時刻の進みを確認した（CoreS3 は以前に設定された正しい時刻を保持していた）。オーディオ出力（`kumiki-class::Speaker`、CoreS3 の AW88298 + esp-hal の I2S DMA ストリーム、DT から生成）で、CoreS3 から音が出ることを確認した。マイク（`kumiki-class::Microphone`、CoreS3 の ES7210）も入れ、声でレベルが動くことを確認した。I2S1 を全二重（RX が TX の BCK / WS を共有）で使い、スピーカーとマイクを同時に動かす。残り：入力のフレーム数が設定の 48kHz より約 3% 少ない（46.6k/s）原因の切り分け、Tab5 のオーディオ（P4 v1 の I2S と ES8388 / ES7210） |

### 実機で分かったこと

- **AtomS3 の判別は IMU に頼らない**：手元の個体は IMU の小基板の SDA が本体とつながっておらず（内部プルダウンで Low、プルアップで High = 線が浮いている）、M5Unified でも IMU が見えなかった。M5GFX と同じくパネル ID（GC9107 の RDDID）で判別する。
- **I2C バスリカバリ**：IMU などは MCU のリセット後も電源が入ったままなので、内部 I2C の初期化前に SCL を最大 9 クロック出してから STOP を出す。
- **Octal PSRAM 内蔵チップでは GPIO33〜37 に触らない**：eFuse の `PSRAM_CAP` で判定し、該当する判別候補（CoreS3 / AtomS3）を飛ばす（M5GFX の `touches_opi_pins` と同じ考え方）。
- **StopWatch のタッチ**：個体によって CST820 と CST9217 がある。CST820 の座標は、M5GFX の設定（0〜233 を 2 倍）とは合わなかった。3 点の実測値から、Linux DT の `touchscreen-min-*` / `touchscreen-size-*` と同じ形の補正値を求めた。補正値は DTS に書けるようにする（[07](07-design-hwdesc.md)）。
- **esp-hal の I2S 受信（ESP32-S3）**：`I2sRxDmaTransfer::is_done()` は、一度立つと消えない RX_DONE の割り込みフラグを読むので、ストリームが止まったかどうかの判定には使えない（「一定時間データが来ない」で判定する）。また `DmaRxStreamBuf` は、DMA の先読みを考えて区切り（ディスクリプタ）を細かくしないと、空きがあっても DMA が止まる（16000 バイトを 4 区切りでは止まり、16 区切りで連続した）。
- **小さな描画の固定費**：esp-hal の DMA 転送の起動には数 µs かかるため、64 バイト以下は SPI の FIFO から直接送る（`fifo-esp32s3`）。

## 確認していただきたい事項

現時点ではなし。
