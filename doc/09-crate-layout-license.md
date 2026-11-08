# 09. クレート構成・依存の切り方・ライセンス

決定事項（2026-11-08）: **クレート名に M5 を付けない。** M5Stack 固有のものも含め、すべて汎用の名前にする。M5Stack の製品は、ボード定義（DTS）とボード名（`board-m5stack-cores3` などの feature）にだけ現れる。

プロジェクト全体の接頭辞は、仮に **`kumiki`**（組木。部品をはめ合わせてボードを組むという意味）とする。crates.io で `kumiki`、`kumiki-gfx`、`kumiki-dt` が未使用であることを確認した（2026-11-08）。なお、当初の候補だった `hwd` と `gfx-core` は既に使われていた。DT の独自プロパティの vendor prefix も `kumiki,` にそろえる。名前を変える場合は一括で置き換える。

## 1. ワークスペースの構成

```
(リポジトリ)/                    (cargo workspace)
├─ gfx/
│  ├─ kumiki-gfx            HAL 非依存。ピクセル形式、PixelSource、Bus/Panel トレイト、
│  │                        Surface（Display/Sprite/FrameBuffer）、FastDraw、DirtyRegions
│  ├─ kumiki-gfx-panels     DcsPanel + Model（ILI9342C, GC9107, ST7789, ST7735, GC9A01, ...）、
│  │                        DSI パネル（ILI9881C, ST7123）、EPD（IT8951 など）
│  ├─ kumiki-gfx-bus-ehal   embedded-hal / embedded-hal-async による汎用のバス実装
│  ├─ kumiki-gfx-bus-esp    esp-hal 依存（SPI DMA パイプライン、I8080、I2S パラレル、DPI、DSI）
│  └─ kumiki-gfx-slint      Slint の LineBufferProvider アダプタ（任意）
├─ dt/                      ハードウェア記述層（詳細は 07）
│  ├─ kumiki-dt             DTS の前処理、構文解析、合成、検証のライブラリ（ホスト上で動く、std）
│  ├─ kumiki-dtgen          バインディングを使って Rust コードを生成する（build.rs 用のライブラリと CLI）
│  ├─ kumiki-dt-runtime     生成コードが使う実行時の部品（no_std。プローブ、判別の決定木、AnyX の補助）
│  ├─ kumiki-class          デバイスクラスのトレイト（Display, Touch, Imu, Rtc, Power, Rail, Audio, Buttons, ...）
│  └─ kumiki-bindings       compatible とドライバをつなぐコード（ctor）。bindings/*.yaml と対になる
├─ hal/
│  ├─ kumiki-hal-esp        esp-hal 用の HAL アダプタ（コントローラの生成、eFuse の読み出し）
│  ├─ kumiki-p4v1-hal       ESP32-P4 rev v1 用の最小限の HAL（tab5-os 由来。embedded-hal を実装し、
│  │                        DSI、GDMA の LLI リング、PPA/DMA2D、PSRAM の初期化を持つ）
│  └─ kumiki-hal-rp         embassy-rp 用の HAL アダプタ（汎用性の確認用）
├─ drivers/                 不足しているドライバ（それぞれ独立に公開する。eh1 + async）
│  ├─ mpu6886, bmm150, aw88298, es8388, ip5306, pi4ioe5v6408, rx8130ce, ltr553, st7123-touch
│  └─ ...
├─ boards/
│  ├─ dts/                  SoC の .dtsi、ボードの .dts、オーバーレイ（07 §3）
│  ├─ bindings/             バインディングの YAML
│  └─ kumiki-boards         DTS から生成したボード定義の集まり（チップの feature とボードの feature を持つ。08）
├─ bench/                   ベンチマーク（Rust 版と、M5GFX 版の Arduino スケッチ）
└─ examples/
```

## 2. HAL 依存の切り方

| クレート | embedded-hal | HAL | 非同期ランタイム | alloc |
|---|---|---|---|---|
| kumiki-gfx / kumiki-gfx-panels | トレイトだけ（`DelayNs`, `OutputPin`） | × | × | 任意（`alloc` feature で Vec ベースのスプライト） |
| kumiki-gfx-bus-ehal | ✓（1.0 と async） | × | × | × |
| kumiki-gfx-bus-esp | ✓ | esp-hal（`requires-unstable`、チップごとの feature） | × | × |
| kumiki-class / kumiki-dt-runtime | トレイトだけ | × | ×（`embassy-sync` は executor に依存しない） | × |
| kumiki-dt / kumiki-dtgen（ホストで動く） | – | – | – | std |
| kumiki-hal-esp / kumiki-p4v1-hal | ✓ | esp-hal / 自前 | × | × |
| kumiki-boards | ✓ | チップの feature で選ぶ | ×（`embassy` / `rtic` feature は補助のラッパーだけ） | 任意 |
| drivers/* | ✓（1.0 と async） | × | × | × |

- **非同期ランタイムには依存しない**（決定事項。[07](07-design-hwdesc.md) §7）。executor、`embassy-time`、タスクの起動は、どのクレートの必須依存にもしない。
- **esp-hal の版を `=` で固定しない。** 範囲指定（例: `>=1.2, <1.4`）にし、互換性を CI の行列テストで確かめる。既存の M5 向けクレートが互いに組み合わせられない原因はここにある（[04](04-ecosystem-bsp.md) §2）。
- esp-hal の unstable API への依存は、kumiki-gfx-bus-esp と kumiki-hal-esp に閉じ込める。上流の API が変わったときに直す場所を限定するためである。
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

- **決定事項: 本プロジェクトは MIT OR Apache-2.0 のデュアルライセンスにする**（Rust の標準）。Rust 組み込みの慣例であり、esp-hal、embedded-graphics、stackchan-rs、tab5-os とも揃う。
- 移植や参照によって取り込むものと、その扱い

| 取り込むもの | 元のライセンス | 扱い |
|---|---|---|
| M5GFX 由来（初期化表、判別手順、M5 固有のコード） | MIT（M5Stack） | `THIRD_PARTY_NOTICES.md` に著作権表示を載せ、該当ファイルの先頭に出典を書く |
| LovyanGFX 由来（lgfx/v1 のアルゴリズムの移植） | BSD-2-Clause（FreeBSD）。一部は TFT_eSPI（BSD-2）と Adafruit（MIT/BSD）に由来 | 同上。バイナリで配布する場合も表示が必要 |
| M5Unified 由来（PMIC の設定手順、ピンの表） | MIT | 同上 |
| mipidsi 由来（初期化表） | MIT | 同上 |
| tab5-os、stackchan-rs 由来 | MIT OR Apache-2.0 | そのまま取り込める |
| Zephyr のバインディングとボード DTS（参照・流用する場合） | Apache-2.0 | 出典を明記し、Apache-2.0 の NOTICE の扱いに従う |
| GPL のもの（epdpi、lilygo-epd47） | GPL-3.0 | **取り込まない**。PaperS3 の EPD はクリーンルームで実装する |
| Slint | GPL-3.0 / 商用 | 依存の必須にしない。アダプタクレートだけを用意し、利用者が自分でライセンスを選ぶ |
| フォント（efont、IPA） | BSD-3 / IPA Font License | 同梱しない。u8g2-fonts などの外部クレートを使ってもらう |

- ボードの DTS は、ハードウェアの事実（ピン番号、アドレス）を記述したものとして、データシートや公式ドキュメントを出典として各ノードのコメントに明記する。
