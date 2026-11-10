# no_std Rust 向けグラフィックドライバ / ハードウェア記述層 / ボード定義の検討

調査日: 2026-11-08。M5GFX と M5Unified に相当するものを no_std Rust で作るための検討結果をまとめる。最初の対象は ESP32 系と M5Stack 製品だが、**汎用の仕組みとして設計する**。プロジェクト名は `kumiki` とする。**実装はまだ行っていない（確認待ち）。**

## 決定事項（2026-11-08）

| 項目 | 決定 |
|---|---|
| ハードウェア記述の構文 | Device Tree Source（DTS） |
| クレート名 | M5 を付けない。プロジェクト名と接頭辞は `kumiki` |
| 非同期 | async を既定にする。ランタイムは embassy に固定しない |
| Tab5 | rev v1（esp-hal は使えないので、自前の HAL で対応する） |
| ライセンス | MIT OR Apache-2.0 |
| 外部との連携 | 行わない。よく使うボードの定義から始める |
| ボードの順序 | CoreS3 / AtomS3 → Tab5。無印 ESP32 系は対象外 |

## 文書一覧

| # | 文書 | 内容 |
|---|---|---|
| 01 | [M5GFX の内部](01-m5gfx-internals.md) | M5GFX と LovyanGFX が性能を出している仕組み（DMA パイプライン、変換、スプライト、機種の判別） |
| 02 | [手元の既存資産](02-local-assets.md) | stackchan-rs、tab5-os、lgfx-rs などの調査と教訓 |
| 03 | [グラフィックのエコシステム](03-ecosystem-graphics.md) | embedded-graphics、mipidsi、display-interface、Slint/LVGL、esp-hal（SPI DMA、LCD_CAM、DSI） |
| 04 | [BSP とドライバのエコシステム](04-ecosystem-bsp.md) | M5 向けクレート、電源、タッチ、IMU、オーディオ、バスの共有、機種ごとの構成 |
| 05 | [新規か拡張か](05-build-vs-contribute.md) | 部品ごとの方針 |
| 06 | [グラフィック層の設計](06-design-graphics.md) | ピクセル形式、バスとパネル、拡張トレイト、スプライト、async |
| 07 | [ハードウェア記述層（kumiki-dt）の設計](07-design-hwdesc.md) | Device Tree ベースの記述、バインディング、コード生成、ランタイム非依存、P4 v1 |
| 08 | [ボード定義と BSP の設計](08-design-bsp.md) | ボードの判別、API、バスの共有、初期化順序、電源、最初の対象ボード |
| 09 | [クレート構成とライセンス](09-crate-layout-license.md) | ワークスペース、HAL 依存の切り方、alloc、ライセンス |
| 10 | [性能目標と検証方法](10-performance-plan.md) | 理論上限、目標、ベンチマークの項目 |
| 11 | [マイルストーン](11-roadmap.md) | 決定事項、M0〜M5、リスク、確認事項 |
| 12 | [M4: Tab5 の計画と記録](12-m4-tab5-plan.md) | P4 v1 の HAL、表示、I2S とコーデック、実機で分かったこと |
| 13 | [電源管理の設計（検討）](13-design-power.md) | 現状、Linux・Zephyr・RTOS の調査、参照カウントとガードによる方針、段階分け |
| 14 | [アプリに渡す資源と追加ボード](14-app-resources.md) | 使わないピン・ペリフェラル（`free` / `spare`）、共有 I²C、非同期の音声、Stack-chan の台座 |

## 結論の要約

### 現状

- **no_std で M5GFX に相当するものは存在しない。**
  - 既存の M5GFX/LovyanGFX のバインディング（lgfx-rs、m5unified-rs）は、どれも std（esp-idf）前提である。
  - 標準的な構成（embedded-graphics 0.8 + mipidsi 0.10）は、ピクセル単位のイテレータとブロッキング SPI で動いている。DMA との重ね合わせも async もない（[03](03-ecosystem-graphics.md)）。
- 下回りの材料は揃っている。
  - esp-hal 1.2.2 は、SPI DMA の非ブロッキング転送（`write_buffer`）、S3 の LCD_CAM（i8080/RGB）、P4 の MIPI-DSI を持つ。ただしどれも unstable 扱いで、P4 は rev v3 以降のみ（手元の Tab5 は rev v1 なので使えない）、PPA は未対応。
  - 手元の stackchan-rs（DMA SPI）と tab5-os（P4 v1 の DSI と PPA）が実証済みの参考になる（[02](02-local-assets.md)）。
- BSP は 1 機種専用のクレートが点在しているだけで、esp-hal の版を厳密に固定しているので組み合わせられない。no_std Rust には、ボードの構成を記述する標準的な層がない（[04](04-ecosystem-bsp.md)、[07](07-design-hwdesc.md)）。

### 方針

1. **グラフィック**（[06](06-design-graphics.md)）
   - embedded-graphics の `DrawTarget` との互換を保ち、速い経路を拡張トレイト `FastDraw` で足す。
   - パネル（コマンドの手順）とバス（転送）を分け、ピクセル変換をバスに渡して DMA と重ねる（M5GFX の方式を、Rust の所有権で安全に表す）。
   - バスは、embedded-hal / embedded-hal-async による汎用実装と、ESP 専用の DMA パイプライン実装の 2 系統を用意する。
2. **ハードウェア記述層 kumiki-dt**（[07](07-design-hwdesc.md)）
   - DTS でボードを記述し、ビルド時に型付きの Rust コードを生成する。
   - 標準の DT の表現（バスの木、phandle、レギュレータ、`*-gpios`、`chosen`、オーバーレイ）をそのまま使い、独自の要素は `kumiki,` の付いたプロパティとノードに限る。
   - 部品の差し替えは、Linux 6.13 の `status = "fail-needs-probe"` の慣例で表す。参照関係まで変わるボードの違いは、DTS を分けたうえで判別ルールのノードで表す。
   - 初期化順序は依存グラフから生成し、不正な記述は生成時にエラーにする。
   - HAL アダプタを挟むので、esp-hal 以外（P4 v1 用の自前 HAL、embassy-rp）でも使える。
   - 非同期ランタイムには依存しない（executor から独立した `embassy-sync` と、`embedded-hal-async` のみを使う。タスクの起動はアプリに任せる）。
3. **ボード定義と BSP**（[08](08-design-bsp.md)）
   - チップは feature で指定し、チップ内のボードは実行時に自動判別する（ボードの feature で固定することもできる）。
   - API は、分割して所有できる `Devices` 構造体として返す。
4. **既存クレートとの切り分け**（[05](05-build-vs-contribute.md)）
   - そのまま使う: embedded-graphics、esp-hal、embedded-hal-bus、embassy-embedded-hal、okhsunrog 氏の `*-dd` 系ドライバ、gt911
   - 新しく作る: グラフィックのバス・パネル・スプライト、kumiki-dt、ボード定義、P4 v1 用の HAL、不足しているドライバ
5. **ライセンス**: MIT OR Apache-2.0。M5GFX（MIT）と LovyanGFX（BSD-2）から移植するものは、著作権表示を保持する（[09](09-crate-layout-license.md)）。

### 最初のマイルストーン（M2）

1 つのバイナリが CoreS3 と AtomS3 の両方で動き、全画面の DMA 転送で M5GFX の 80% 以上の fps を出すこと。同じアプリが embassy とそれ以外の実行環境の両方で動くことも条件にする。40MHz SPI での理論上限は、320×240 で 32.6fps、128×128 で 152fps である。DTS からの生成器は、M2 の手書きのコードを再現する形で M3 に作る（[10](10-performance-plan.md)、[11](11-roadmap.md)）。

### 状況

確認事項はすべて決定済み。M0（土台）から実装に着手した。
