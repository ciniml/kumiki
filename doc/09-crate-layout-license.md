# 09. クレート構成・依存の切り方・ライセンス

クレート名はすべて仮称。M5 は M5Stack の商標なので、汎用のグラフィック層と記述層には M5 を冠さない名前を付ける。M5 固有の部分だけを `m5-*` にする。名前は公開前に決める（[11](11-roadmap.md) の未決事項）。

## 1. ワークスペースの構成

```
m5-rs/                          (cargo workspace)
├─ gfx/
│  ├─ gfx-core        HAL 非依存。ピクセル形式、PixelSource、Bus/Panel トレイト、
│  │                  Surface（Display/Sprite/FrameBuffer）、FastDraw、DirtyRegions
│  ├─ gfx-panels      DcsPanel + Model（ILI9342C, GC9107, ST7789, ST7735, GC9A01, ...）、
│  │                  DSI パネル（ILI9881C, ST7123）、EPD（IT8951 など）
│  ├─ gfx-bus-ehal    embedded-hal / embedded-hal-async による汎用のバス実装
│  ├─ gfx-bus-esp     esp-hal 依存（SPI DMA パイプライン、I8080、I2S パラレル、DPI、DSI）
│  ├─ gfx-esp32p4v1   P4 rev v1.x 向けのレジスタ直叩き（DSI, GDMA リング, PPA/DMA2D）
│  └─ gfx-slint       Slint の LineBufferProvider アダプタ（任意）
├─ hwd/                         ハードウェア記述層（詳細は 07）
│  ├─ hwd-model       記述のデータモデルとスキーマ（no_std、const で扱えるデータ型）
│  ├─ hwd-gen         記述ファイルとバインディングから Rust コードを生成する（build.rs / CLI）
│  ├─ hwd-runtime     生成コードが使う実行時の部品（プローブ手順、初期化順の実行、AnyX の生成補助）
│  └─ hwd-class       デバイスクラスのトレイト（Display, Touch, Imu, Rtc, Power, Audio, Buttons, ...）
├─ drivers/                     不足しているドライバ（それぞれ独立に公開、eh1 + async）
│  ├─ mpu6886, bmm150, aw88298, es8388, ip5306, pi4ioe5v6408, rx8130ce, ltr553, st7123-touch
│  └─ ...
├─ boards/
│  ├─ descriptions/   機種ごとの記述ファイル（m5stack/*.dts など）とオーバーレイ（Unit、Module）
│  ├─ m5-bsp          記述から生成した BSP。チップごとの feature と機種判別を持つ（詳細は 08）
│  └─ (generic-bsp)   M5 以外のボード（ESP32-S3-DevKitC + SPI LCD、RP2040 + ST7789 など）で
│                     汎用性を確かめるための見本
├─ bench/             ベンチマーク（Rust 版と、M5GFX 版の Arduino スケッチ）
└─ examples/
```

## 2. HAL 依存の切り方

| クレート | embedded-hal | esp-hal | embassy | alloc |
|---|---|---|---|---|
| gfx-core / gfx-panels | トレイトだけ（`DelayNs`, `OutputPin`）。任意 | × | × | 任意（`alloc` feature で Vec ベースのスプライト） |
| gfx-bus-ehal | ✓（1.0 と async） | × | × | × |
| gfx-bus-esp | ✓ | ✓（`requires-unstable`、チップごとの feature） | 任意（async） | × |
| hwd-model / hwd-class | トレイトだけ | × | × | × |
| hwd-gen（ホストで動く） | – | – | – | std |
| m5-bsp | ✓ | ✓ | ✓（既定）。ブロッキング版は feature で選ぶ | 任意（PSRAM 上のスプライトを使うなら必要） |
| drivers/* | ✓（1.0 と async） | × | × | × |

- **esp-hal の版を `=` で固定しない。** 範囲指定（例: `>=1.2, <1.4`）にし、互換性を CI の行列テストで確かめる。既存の M5 向けクレートが互いに組み合わせられない原因はここにある（[04](04-ecosystem-bsp.md) §2）。
- esp-hal の unstable API への依存は gfx-bus-esp と m5-bsp に閉じ込める。上流の API が変わったときに直す場所を限定するためである。
- `defmt` と `log` はどちらも任意の feature にする。

## 3. no_std と alloc

- すべてのランタイムクレートは `#![no_std]` にする。
- **alloc がなくても使えることを必須にする。** バッファは `&'static mut [u8]`（`StaticCell` など）で渡せるようにする。
- `alloc` feature を有効にすると、PSRAM や内部 RAM の DMA 可能な領域から確保できるようにする。esp-alloc のメモリ種別ごとのアロケータに対応させることを検討する（esp-alloc が `allocator_api2` に対応しているかは実装前に確かめる。現時点では未確認）。
- 大きなバッファは Wi-Fi を起動する前に確保することを、BSP の初期化 API の形で促す（[02](02-local-assets.md) の教訓 4）。

## 4. ツールチェイン

- Xtensa（ESP32 と S3）: espup の `esp` ツールチェインと GCC 15.2 系のリンカ（古い 8.4 系は使わない。[02](02-local-assets.md)）。
- RISC-V（C3、C6、P4）: 安定版 Rust。
- MSRV は esp-hal に合わせる（1.2.2 は rust-version 1.95）。

## 5. ライセンス

- **本プロジェクトは MIT OR Apache-2.0 のデュアルライセンスにする。** Rust 組み込みの慣例であり、esp-hal、embedded-graphics、stackchan-rs、tab5-os とも揃う。
- 移植や参照によって取り込むものと、その扱い

| 取り込むもの | 元のライセンス | 扱い |
|---|---|---|
| M5GFX 由来（初期化表、判別手順、M5 固有のコード） | MIT（M5Stack） | `THIRD_PARTY_NOTICES.md` に著作権表示を載せ、該当ファイルの先頭に出典を書く |
| LovyanGFX 由来（lgfx/v1 のアルゴリズムの移植） | BSD-2-Clause（FreeBSD）。一部は TFT_eSPI（BSD-2）と Adafruit（MIT/BSD）に由来 | 同上。バイナリで配布する場合も表示が必要 |
| M5Unified 由来（PMIC の設定手順、ピンの表） | MIT | 同上 |
| mipidsi 由来（初期化表） | MIT | 同上 |
| tab5-os、stackchan-rs 由来 | MIT OR Apache-2.0 | そのまま取り込める |
| GPL のもの（epdpi、lilygo-epd47） | GPL-3.0 | **取り込まない**。PaperS3 の EPD はクリーンルームで実装する |
| Slint | GPL-3.0 / 商用 | 依存の必須にしない。アダプタクレートだけを用意し、利用者が自分でライセンスを選ぶ |
| フォント（efont、IPA） | BSD-3 / IPA Font License | 同梱しない。u8g2-fonts などの外部クレートを使ってもらう |

- 機種の記述ファイル（DTS など）は、ハードウェアの事実（ピン番号、アドレス）を記述したもので、データシートや公式ドキュメントを出典として明記する。M5GFX の `board_spec/*.json` から機械的に変換する場合は、MIT の表示を付ける。
