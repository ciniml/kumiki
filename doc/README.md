# no_std Rust 向けグラフィックドライバ / ハードウェア記述層 / BSP の検討

調査日: 2026-11-08。M5GFX と M5Unified に相当するものを no_std Rust で作るための検討結果をまとめる。最初の対象は ESP32 系と M5Stack 製品だが、汎用の仕組みとして設計する。**実装はまだ行っていない（確認待ち）。**

## 文書一覧

| # | 文書 | 内容 |
|---|---|---|
| 01 | [M5GFX の内部](01-m5gfx-internals.md) | M5GFX と LovyanGFX が性能を出している仕組み（DMA パイプライン、変換、スプライト、機種の判別） |
| 02 | [手元の既存資産](02-local-assets.md) | stackchan-rs、tab5-os、lgfx-rs などの調査と教訓 |
| 03 | [グラフィックのエコシステム](03-ecosystem-graphics.md) | embedded-graphics、mipidsi、display-interface、Slint/LVGL、esp-hal（SPI DMA、LCD_CAM、DSI） |
| 04 | [BSP とドライバのエコシステム](04-ecosystem-bsp.md) | M5 向けクレート、電源、タッチ、IMU、オーディオ、バスの共有、機種ごとの構成 |
| 05 | [新規か拡張か](05-build-vs-contribute.md) | 部品ごとの方針 |
| 06 | [グラフィック層の設計](06-design-graphics.md) | ピクセル形式、バスとパネル、拡張トレイト、スプライト、async |
| 07 | [ハードウェア記述層（HWD）の設計](07-design-hwdesc.md) | Device Tree や ACPI に相当する層。データモデル、バインディング、コード生成 |
| 08 | [BSP の設計](08-design-bsp.md) | 機種の判別、API、バスの共有、初期化順序、電源 |
| 09 | [クレート構成とライセンス](09-crate-layout-license.md) | ワークスペース、HAL 依存の切り方、alloc、ライセンス |
| 10 | [性能目標と検証方法](10-performance-plan.md) | 理論上限、目標、ベンチマークの項目 |
| 11 | [マイルストーン](11-roadmap.md) | M0〜M5、リスク、未決事項 |

## 結論の要約

### 現状

- **no_std で M5GFX に相当するものは存在しない。**
  - 既存の M5GFX/LovyanGFX のバインディング（lgfx-rs、m5unified-rs）は、どれも std（esp-idf）前提である。
  - 標準的な構成（embedded-graphics 0.8 + mipidsi 0.10）は、ピクセル単位のイテレータとブロッキング SPI で動いている。DMA との重ね合わせも async もない（[03](03-ecosystem-graphics.md)）。
- 下回りの材料は揃っている。
  - esp-hal 1.2.2 は、SPI DMA の非ブロッキング転送（`write_buffer`）、S3 の LCD_CAM（i8080/RGB）、P4 の MIPI-DSI を持つ。ただしどれも unstable 扱いで、P4 は rev v3 以降のみ、PPA は未対応。
  - 手元の stackchan-rs（DMA SPI）と tab5-os（P4 v1 の DSI と PPA）が実証済みの参考になる（[02](02-local-assets.md)）。
- BSP は 1 機種専用のクレートが点在しているだけで、esp-hal の版を厳密に固定しているので組み合わせられない。ボードの構成を記述する標準的な方法はない（[04](04-ecosystem-bsp.md)）。
- 一方で、ボードの構成をデータで記述する先例は、最近になって増えている（[07](07-design-hwdesc.md)）。
  - Zephyr DT とその Rust 対応、Hubris の app.toml、Ariel OS の sbd
  - **Espressif の esp-board-manager**（YAML、M5 製品を約 35 機種収録）
  - **M5GFX の board_spec**（JSON。リビジョンごとの部品の差し替えと、判別順序の生成を扱う）

### 方針

1. **グラフィック**: embedded-graphics の `DrawTarget` との互換を保ち、速い経路を拡張トレイト `FastDraw` で足す。パネル（コマンドの手順）とバス（転送）を分け、ピクセル変換をバスに渡して DMA と重ねる（M5GFX の方式を Rust の所有権で安全に表す）。バスは embedded-hal / embedded-hal-async による汎用実装と、ESP 専用の DMA パイプライン実装の 2 系統を用意する（[06](06-design-graphics.md)）。
2. **ハードウェア記述層（HWD）**
   - DT のデータモデル（ノード、compatible、reg、参照）に、M5GFX の board_spec の考え方（choices / revisions / detect / provenance）を足す。
   - 記述ファイルから、**ビルド時に型付きの Rust コードを生成**する。
   - ドライバとのつなぎは、HAL アダプタと、embedded-hal で型付けしたバインディングで行う。初期化順序は依存グラフから生成し、不正な記述は生成時にエラーにする。
   - 実行時の機種判別は、「生成された候補から選ぶ」形に限定する（[07](07-design-hwdesc.md)）。
3. **BSP**
   - チップは feature で指定し、チップ内の機種は実行時に自動判別する（機種の feature で固定することもできる）。
   - API は、分割して所有できる `Devices` 構造体として返す。各フィールドはデバイスクラスのトレイトを実装し、機種によって存在しないものは `Option` にする（[08](08-design-bsp.md)）。
4. **既存クレートとの切り分け**（[05](05-build-vs-contribute.md)）
   - そのまま使う: embedded-graphics、esp-hal、embedded-hal-bus、embassy-embedded-hal、okhsunrog 氏の `*-dd` 系ドライバ、gt911
   - 上流に貢献する: esp-hal、embedded-graphics、既存ドライバの async 対応
   - 新しく作る: グラフィックのバス・パネル・スプライト、HWD、BSP、不足しているドライバ（MPU6886、AW88298、ES8388、PI4IOE5V6408 など）
5. **ライセンス**: MIT OR Apache-2.0。M5GFX（MIT）と LovyanGFX（BSD-2）から移植するものは、著作権表示を保持する。GPL の EPD ドライバは取り込まない（[09](09-crate-layout-license.md)）。

### 最初のマイルストーン（M2）

1 つのバイナリが CoreS3 と AtomS3 の両方で動き、全画面の DMA 転送で M5GFX の 80% 以上の fps を出すこと。40MHz SPI での理論上限は 320×240 で 32.6fps、128×128 で 152fps である。HWD の生成器は、M2 の手書きのコードを再現する形で M3 に作る（[10](10-performance-plan.md)、[11](11-roadmap.md)）。

### 確認していただきたい事項

[11](11-roadmap.md) の末尾を参照。主なものは次の 3 つ。

- HWD の構文（TOML か DTS か）
- クレートの名前
- 手元の Tab5 の silicon rev
