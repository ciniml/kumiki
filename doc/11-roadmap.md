# 11. マイルストーンと計画

## 進め方の方針

- **コード生成器は、手書きのコードを先に作ってから作る。** M2 で「生成されるはずの形」のコードを CoreS3 と AtomS3 向けに手で書き、M3 でそれを再現する生成器を作る（生成結果をゴールデンファイルとして比較する）。生成器を先に作ると、実際のドライバの組み合わせで困る点（所有権、共有バス、ピンの型）を後から知ることになり、作り直しが増えるためである。
- 各マイルストーンの終わりに、ベンチマークの結果を `doc/bench-results/` に残す。

## マイルストーン

### M0: 土台と基準値

- cargo のワークスペース、CI を用意する。
  - ビルドの対象: esp32、esp32s3、esp32c6、thumbv6m（RP2040）
  - CI で回すもの: fmt、clippy、host 上のテスト
- ツールチェインの手順を文書化する（espup、GCC 15.2 系のリンカ）。
- **M5GFX のベンチマークのスケッチ**（B1〜B7、[10](10-performance-plan.md)）を作り、CoreS3 と AtomS3 で基準値を取る。
- 完了の条件: M5GFX の基準値の表ができていること。

### M1: グラフィックの中核

- `gfx-core`: ピクセル形式、`PixelSource`、Bus/Panel のトレイト、`Display`、`Sprite`（等倍コピーと透過）、`FastDraw`
- `gfx-panels`: `DcsPanel` + ILI9342C、GC9107（ST7789 も入れられれば入れる）
- `gfx-bus-esp`: `EspSpiDmaBus`（`write_buffer` を使った A/B のパイプライン、`write_repeat`）
- `gfx-bus-ehal`: `SpiDeviceBus` と `AsyncSpiBus`
- B1〜B4 と B8 を測る。
- 完了の条件: CoreS3 で B2 が M5GFX の 80% 以上。

### M2: 最初のマイルストーン（CoreS3 と AtomS3 で同じアプリが動く）

- `hwd-class`（Display、Touch、Power、Buttons、Imu の最小限のトレイト）
- `m5-bsp`（`chip-esp32s3`）を、生成されるはずの形で**手書き**する。
  - 機種の判別（AXP2101 + AW9523 があれば CoreS3、なければ GC9107 の RDDID を読んで AtomS3）
  - CoreS3: AXP2101 と AW9523 による電源投入、GPIO35 の切り替え、FT6336U、BMI270
  - AtomS3: PWM のバックライト、GPIO41 のボタン、MPU6886（ドライバを新しく書く）
  - ボタンは共通のビットマスクで扱う（CoreS3 は電源キーとタッチ領域、AtomS3 は GPIO）
- `examples/same-app`: **1 つのバイナリ**が CoreS3 と AtomS3 の両方で起動する。画面の大きさに合わせて描画内容を変え、ボタンかタッチで画面を切り替え、IMU の値を表示する。
- **完了の条件**
  1. 同じバイナリ（`chip-esp32s3` で、機種の feature は指定しない）が両方の機種で動く。
  2. 全画面の DMA 転送（B2）が、両方の機種で M5GFX の **80% 以上**の fps を出す。
  3. B1、B3、B4 も 80% 以上を出す。B5、B9 は測定して記録する。
  4. blocking 版と async（embassy）版の両方の見本がある。

### M3: HWD 生成器 v0

- TOML のスキーマ（`hwd-model`）、バインディングのマニフェスト、`hwd-gen`（`check / gen / pintable`）
- **M2 の手書きのコードを再現**し、ゴールデンファイルで比較する。
- 記述を追加するだけで Cardputer と AtomS3R に対応できることを示す。ここで手書きのコードは書かない（HWD の価値を確かめるため）。
- 汎用性の確認として、ESP32-S3-DevKitC + ST7789、RP2040 + ST7789（`hwd-hal-rp`）も記述して動かす。
- M5GFX の board_spec から TOML に変換する取り込み器（試作）を作る。

### M4: ESP32 系への展開と機能の拡充

- `chip-esp32`: Core2（v1.0、v1.1、v1.3 のロット違いを `choices` で吸収）、StickC Plus2
- IMU、RTC、オーディオ（NS4168、AW88298、ES7210）
- スプライトの回転・拡縮、パレット、`DirtyRegions`、ダブルバッファ
- Slint のアダプタ
- 無印 ESP32 で PSRAM 上のスプライトを転送する経路（パイプライン）の最適化

### M5: Tab5 と EPD

- Tab5
  - rev v3 の silicon には esp-hal の `mipi_dsi` を使う。
  - rev v1 の silicon には、tab5-os のレジスタ直叩き実装（DSI、GDMA リング、PPA）を移植する。
  - パネルのロット違い（ILI9881C、ST7123、ST7121）を判別する。
  - PPA を `FramebufferPanel::accel` として使う。
- Paper（IT8951）と PaperS3（パラレル EPD をクリーンルームで実装）

## 主なリスク

| リスク | 対策 |
|---|---|
| esp-hal の unstable API が変わる | 依存を gfx-bus-esp と m5-bsp に閉じ込める。版は範囲で指定し、CI の行列テストで確かめる |
| esp-hal の `write_buffer` の固定費が大きく、M5GFX の 80% に届かない | 小さな転送は FIFO へ直接書く経路を持つ。最終手段として、SPI のレジスタを直接叩く経路を unsafe で用意する（M5GFX と同じ方式） |
| P4 の rev v1 は esp-hal が対応しない | tab5-os の実装を移植する（M5） |
| HWD のスキーマが早い段階で固まりすぎる | M3 では v0 として破壊的変更を許す。M4 で機種を増やしてから v1 とする |
| ドライバの品質のばらつき | 実機で検証したものだけを既定のバインディングにする。provenance で区別する |

## 確認していただきたい事項（未決事項）

1. **HWD の構文**: TOML（推奨）にするか、DTS の一部にするか（[07](07-design-hwdesc.md) §4）
2. **プロジェクト名とクレート名**: グラフィックと HWD の層には M5 を冠さない名前を付ける案でよいか（[09](09-crate-layout-license.md)）
3. **既定は async（embassy）優先**で、blocking も同等に対応する方針でよいか
4. **手元の Tab5 の P4 の silicon rev**（v1.x か v3.x か）。M5 の進め方が変わる
5. **ライセンス**: MIT OR Apache-2.0 でよいか
6. **外部との連携**（Ariel OS sbd、M5GFX board_spec の担当者への相談）を、どの段階で始めるか
