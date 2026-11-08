# 07. ハードウェア記述層の設計（Device Tree ベース）

> 決定事項（2026-11-08）: 記述の構文には **Device Tree Source（DTS）** を採用する。Linux で長年、Arm を中心とした多様なハードウェアに使われてきて記述力が実証されており、近年は Zephyr でも採用されているためである。以下、この層を **kumiki-dt** と呼ぶ（クレート名の方針は [09](09-crate-layout-license.md)）。

## 0. 問題設定

no_std Rust では、HAL の抽象化（embedded-hal 1.0 / embedded-hal-async）と、チップの記述（esp-metadata、stm32-data）は整っている。しかし、**「このボードでは、どのバスがどのピンにあり、そこにどのデバイスがどのアドレスでつながり、リセット線や電源がどこから来るか」を表す標準的な層がない**。

PC には ACPI があり、Linux や Zephyr には Device Tree がある。no_std Rust では、BSP ごとに手書きのコードでこれを表しているのが現状である（[04](04-ecosystem-bsp.md) §2）。kumiki-dt は、DTS で書いたボードの記述と、HAL のトレイトに対してジェネリックなドライバとを、**ビルド時に生成する型付きの Rust コード**で結び付ける。

```
 アプリ
   │  デバイスクラスのトレイト（Display / Touch / Imu / Power / ...）
 ボード定義（DTS から生成されるコード）  ← 本章
   │  バインディング（compatible → Rust のドライバ型 + 生成方法）
 ドライバ（embedded-hal / embedded-hal-async のトレイトに対してジェネリック）
   │
 HAL（esp-hal, P4 v1 用の自前 HAL, embassy-rp, embassy-stm32, ...）
   │
 チップの記述（esp-metadata など）→ SoC の .dtsi を生成する
```

## 1. 先行事例と教訓

| 方式 | 例 | 本設計での扱い |
|---|---|---|
| A. コードで記述する | Tock の components、atsamd/rp の `bsp_pins!`、assign-resources | 生成されるコードの形として参考にする |
| B. データからビルド時にコード生成する | **Zephyr DT とその Rust 対応**（zephyr-lang-rust）、Hubris の app.toml、Ariel OS の sbd、esp-board-manager、M5GFX の board_spec | **採用する方式** |
| C. 実行時にデータを解析する | Linux の FDT / ACPI | 将来の拡張（§10） |

出典

- Zephyr: <https://docs.zephyrproject.org/latest/build/dts/howtos.html>、<https://github.com/zephyrproject-rtos/zephyr-lang-rust>
- Hubris: <https://github.com/oxidecomputer/hubris/blob/master/build/i2c/src/lib.rs>
- Ariel OS sbd: <https://github.com/ariel-os/sbd>
- esp-board-manager: <https://github.com/espressif/esp-board-manager>
- M5GFX board_spec: <https://github.com/m5stack/M5GFX/tree/master/extras/board_spec>

設計に反映する教訓

1. **バスを親、デバイスを子とする木と、phandle による参照でグラフを表す**（DT そのもの）。リセット線や電源の参照は、初期化順序（依存関係の DAG）でもある。
2. **初期化順序は依存グラフから生成する。** Zephyr では、DT の依存関係と手で付けた初期化の優先度との食い違いがよくある失敗の原因になっており、後から `scripts/build/check_init_priorities.py` で検査するようになった。kumiki-dt では最初から順序をグラフから生成する。
3. **バインディングが見つからない compatible は生成エラーにする**（Hubris）。型付きのアクセサを生成し、デバイスは一度しか取り出せないようにする（zephyr-lang-rust の `get_instance()`、Ariel の `define_peripherals!`）。
4. **I2C アドレスは DT の慣例どおり 7bit で書く**（`reg`）。esp-board-manager の YAML では 7bit と 8bit が混在しており、DT ならこの問題は起きにくい。
5. **ロットごとの部品の違いと実行時の判別を表せること**（M5GFX の board_spec）。DT では、後述する Linux の「second source」の仕組みと、判別ルールを書くノードで表す（§4）。
6. **チップの層とボードの層を分ける**（SoC の `.dtsi` と、ボードの `.dts`）。
7. **命令的な初期化を書ける抜け道が必要**（esp-board-manager の `setup_device.c`）。DT 流に、glue ノード（固有の compatible を持ち、Rust のコードに結び付く）として表す（§6.4）。

## 2. なぜ DTS か、そして DTS の弱点への対処

| 観点 | 評価 |
|---|---|
| 記述力 | バスの木、phandle の参照、`#*-cells` による参照先ごとの引数、レギュレータ、GPIO、割り込み、コネクタ（nexus ノードと `gpio-map`）、オーバーレイ。Linux と Zephyr で実証済み |
| 既存の資産 | Linux の vendor prefix と compatible の命名、レギュレータやタッチスクリーンなどの汎用バインディング、Zephyr の M5Stack ボード定義。例えば m5stack_core2 は `x-powers,axp192-regulator` と `x-powers,axp192-gpio` を使い、表示のリセットを AXP192 の GPIO4 経由で記述している（<https://docs.zephyrproject.org/latest/boards/m5stack/m5stack_core2/doc/index.html>） |
| 学習コスト | Linux や Zephyr の経験者がそのまま読み書きできる |
| 弱点 1: C プリプロセッサへの依存 | `#include` とマクロ（`GPIO_ACTIVE_LOW` など）は使うが、**C プリプロセッサの必要最小限（#include、#define、#if）を Rust で実装**し、外部ツールなしでビルドできるようにする |
| 弱点 2: 型が弱い | バインディング（YAML）でプロパティの型と必須かどうかを宣言し、生成時に検証する（Zephyr と同じ） |
| 弱点 3: 実行時の判別は表せない | 独自の性質は vendor prefix `kumiki,` を付けたプロパティとノードに限定し、標準の構文の範囲で表す（§4） |

標準との互換性: kumiki-dt の DTS は、**標準の `dtc` でもエラーなくコンパイルできること**を CI で確かめる。独自の構文は追加しない。

## 3. ファイル構成

```
boards/dts/
├─ include/kumiki/         gpio.h, irq.h, regulator.h …（定数。値は Linux / Zephyr の dt-bindings と一致させる）
├─ soc/espressif/          esp32.dtsi, esp32s3.dtsi, esp32p4.dtsi …（esp-metadata から生成する。§5.2）
├─ m5stack/
│  ├─ cores3-common.dtsi
│  ├─ cores3.dts           #include "cores3-common.dtsi"
│  ├─ cores3-se.dts        /delete-node/ &camera; など
│  ├─ atoms3.dts, atoms3r.dts, core2.dts, core2-v1_1.dts, tab5.dts, paper.dts …
│  └─ overlays/            unit-env4.dtso, module-xxx.dtso …（Unit と Module を /plugin/ のオーバーレイとして記述する）
└─ generic/                esp32s3-devkitc-st7789.dts, rp2040-pico-st7789.dts（汎用性の確認用）
bindings/                  compatible ごとのバインディング（YAML）
```

## 4. 記述の例（CoreS3 を簡略化したもの）

```dts
/dts-v1/;
#include <soc/espressif/esp32s3.dtsi>
#include <kumiki/gpio.h>
#include <kumiki/irq.h>

/ {
    model = "M5Stack CoreS3";
    compatible = "m5stack,cores3", "espressif,esp32s3";

    chosen {                                   /* 役割（アプリから見た名前）。Zephyr の zephyr,display と同じ考え方 */
        kumiki,display = &lcd;
        kumiki,touch   = &touch;
        kumiki,imu     = &imu;
        kumiki,rtc     = &rtc;
        kumiki,power   = &pmic;
    };

    lcd_backlight: backlight {
        compatible = "kumiki,regulator-backlight";   /* PMIC のレール 1 本で輝度を変える */
        power-supply = <&dldo1>;
    };

    kumiki,detect {                            /* 機種ファミリーの中でこの機種を識別するルール（すべてを満たすこと） */
        compatible = "kumiki,board-detect";
        pmic-id  { compatible = "kumiki,probe-i2c-reg"; device = <&pmic>;  register = <0x03>; expect = <0x4a>; };
        ioexp-id { compatible = "kumiki,probe-i2c-reg"; device = <&ioexp>; register = <0x10>; expect = <0x23>; };
    };
};

&i2c0 {                                        /* 内部 I2C */
    status = "okay";
    clock-frequency = <400000>;
    pinctrl-0 = <&i2c0_int_pins>;              /* i2c0_int_pins: SDA=12, SCL=11（SoC の .dtsi 側で定義する） */

    pmic: pmic@34 {
        compatible = "x-powers,axp2101";
        regulators {                           /* Linux / Zephyr の標準レギュレータバインディング */
            aldo3: ALDO3 { regulator-min-microvolt = <3300000>; regulator-max-microvolt = <3300000>; };
            bldo1: BLDO1 { regulator-min-microvolt = <3300000>; regulator-max-microvolt = <3300000>; };
            dldo1: DLDO1 { regulator-name = "lcd-backlight"; };
        };
    };

    ioexp: gpio@58 {
        compatible = "awinic,aw9523b";
        gpio-controller;
        #gpio-cells = <2>;
        interrupt-controller;
        #interrupt-cells = <2>;
    };

    touch: touchscreen@38 {
        compatible = "focaltech,ft6336u";
        reset-gpios = <&ioexp 0 GPIO_ACTIVE_LOW>;               /* P0_0 */
        interrupts-extended = <&ioexp 10 IRQ_TYPE_EDGE_FALLING>; /* P1_2 */
        touchscreen-size-x = <320>;
        touchscreen-size-y = <240>;
    };

    imu: imu@69 { compatible = "bosch,bmi270"; };
    rtc: rtc@51 { compatible = "belling,bm8563", "nxp,pcf8563"; };   /* DT の互換リスト: 専用の binding がなければ pcf8563 で代用する */
};

&spi2 {
    status = "okay";
    pinctrl-0 = <&spi2_lcd_pins>;              /* SCK=36, MOSI=37, MISO=35 */
    cs-gpios = <&gpio 3 GPIO_ACTIVE_LOW>;

    lcd: display@0 {
        compatible = "ilitek,ili9342c";
        reg = <0>;
        spi-max-frequency = <40000000>;
        dc-gpios = <&gpio 35 GPIO_ACTIVE_HIGH>;                 /* MISO と共用。下の glue ノードで切り替える */
        reset-gpios = <&ioexp 9 GPIO_ACTIVE_LOW>;                /* P1_1 */
        power-supply = <&aldo3>;
        backlight = <&lcd_backlight>;
        width = <320>;
        height = <240>;
        kumiki,invert;
        kumiki,rgb-order = "bgr";
    };

    lcd_dc_mux: pin-share {                    /* glue ノード: Rust で書いた固有の処理に結び付く（§6.4） */
        compatible = "m5stack,cores3-miso-dc-share";
        gpios = <&gpio 35 0>;
    };
};

&i2s0 {
    speaker: amplifier@36 { … };               /* AW88298。I2C 側の制御は &i2c0 の子として書き、相互に参照する */
};
```

### 4.1 ロットや部品の違いを表す 2 つの方法

**(a) 部品の差し替え: Linux の「second source」の慣例に従う。**
Linux 6.13 で取り込まれた I2C の component prober は、候補となる部品のノードに `status = "fail-needs-probe"` を付けておき、起動時にプローブして応答した部品だけを有効にする仕組みである（ChromeOS の機器でタッチパネルやトラックパッドの差し替えに使われている）。

- <https://github.com/torvalds/linux/blob/master/drivers/i2c/i2c-core-of-prober.c>
- パッチの議論: <https://lkml.iu.edu/hypermail/linux/kernel/2410.1/00221.html>

kumiki-dt は同じ表記を採用し、**生成時に候補をすべて含めておき、実行時にプローブして 1 つを選ぶ**コードを生成する。

```dts
&i2c0 {
    imu_a: imu@68 { compatible = "tdk,mpu6886"; status = "fail-needs-probe"; kumiki,probe-group = "imu"; };
    imu_b: imu@68 { compatible = "bosch,bmi270"; status = "fail-needs-probe"; kumiki,probe-group = "imu"; };
};
/ { chosen { kumiki,imu = &imu_a, &imu_b; }; };   /* 候補のうち有効になったもの */
```

  - ノード名（`imu@68`）が重複すると dtc はエラーにするので、実際にはノード名を `imu-mpu6886@68` のように分ける。unit address の重複は警告で済み、有効でないノードについては dtc は警告しない。
  - この方法が向くのは、**他のノードから参照されない部品**の差し替え（IMU、タッチ、パネルのコントローラ違い）である。

**(b) 機種の違い: ボードの DTS を分け、判別ルールを持たせる。**
Core2 の v1.0（AXP192）と v1.1（AXP2101）は、LCD のリセット線の接続先（AXP192 の GPIO4 か、AXP2101 の ALDO2 か）など、**他のノードからの参照まで変わる**。このような違いは `core2.dts` と `core2-v1_1.dts` に分けて書き、共通部分は `.dtsi` にまとめ、それぞれに `kumiki,detect` を書く。複数のボードを 1 つのバイナリにまとめる方法は §6.3。

## 5. バインディングと SoC の記述

### 5.1 バインディング（YAML）

Zephyr のバインディングの書式を土台にし、Rust 用の `kumiki:` セクションを追加する。

```yaml
# bindings/focaltech,ft6336u.yaml
compatible: "focaltech,ft6336u"
include: [i2c-device.yaml, touchscreen-common.yaml]
properties:
  reset-gpios:          { type: phandle-array, required: false }
  interrupts-extended:  { type: phandle-array, required: false }
kumiki:
  class: [touch]                                  # デバイスクラス（kumiki-class のトレイト）
  driver: { crate: "ft6336u-dd", version: "0.2" }
  ctor: "kumiki_bindings::ft6336u::new"           # fn new(i2c, reset: Option<impl OutputPin>, irq: Option<impl Wait>, &Props) -> Result<impl Touch, _>
  provides: []                                    # gpio-controller なら、ここで GPIO を提供することを宣言する
```

- プロパティの型、必須かどうか、`include` による共通化は Zephyr と同じにする。Zephyr のバインディング（Apache-2.0）にあるプロパティ定義は、出典を明記して流用できる。
- `compatible` 文字列は、Linux / Zephyr に既存のものがあればそれを使う（`x-powers,axp2101`、`ilitek,ili9342c`、`focaltech,ft6236` 系など）。ないものだけ、メーカーの vendor prefix で新しく付ける。
- サードパーティのドライバクレートは kumiki のことを知らないので、つなぎのコード（`ctor`）は `kumiki-bindings` クレートにまとめる。zephyr-lang-rust の `dt-rust.yaml`（augment）と同じ考え方である。
- バインディングのない compatible は、生成時にエラーにする。ただし、互換リストの後ろの候補に binding があれば、それを使う。
- 汎用の標準バインディング（`gpio-keys`、`gpio-leds`、`pwm-backlight`、`regulator-fixed`、`regulator-gpio`）にも対応する。これで、M5 以外のボードもそのまま書ける。

### 5.2 SoC の .dtsi（チップの層）

- ESP32 系の `.dtsi` は、**esp-metadata（esp-hal のチップ記述）から生成**する。内容は、ペリフェラルのノード（`i2c0`、`spi2`、`i2s0`、`lcd_cam`、`ledc`、`dsi` など）、`gpio` コントローラ（ESP では GPIO マトリクスでほぼ任意のピンに割り当てられる）、pinctrl の定義。
  - Zephyr の ESP32 用 `.dtsi` は hal_espressif のマクロに依存しているので、そのままは使わず、参照するだけにする。
- pinctrl は、Zephyr の `pinctrl-0` / `pinctrl-names` の構造に合わせつつ、中身は単純な「信号名 = ピン番号」で書く（例: `i2c0_int_pins: i2c0-int { sda = <12>; scl = <11>; bias-pull-up; };`）。
- ピンの検証（存在するか、使えない用途に割り当てていないか、二重に割り当てていないか）は、生成時に esp-metadata のピン情報で行う。
- 他のチップ（RP2040、STM32）用の `.dtsi` は、それぞれのメタデータから作るか、手で書く。

## 6. コード生成

### 6.1 処理の流れ（`kumiki-dtgen`。ホスト上で動く）

1. 前処理: `#include`、`#define`、`#if` を Rust で実装した前処理器で展開する。
2. 構文解析: DTS v1 の構文を解析し、`/delete-node/`、`/delete-property/`、ラベル、オーバーレイ（`/plugin/`、`&label { }`）を処理する。
3. 合成: SoC の `.dtsi`、ボードの `.dts`、オーバーレイの順に重ねる。
4. 検証: バインディングによる型の検査、phandle の解決、ピンの検証、unit address の検査、循環参照の検出を行う。
5. 依存グラフ: `*-supply`、`*-gpios`、`interrupts-extended`、`backlight`、親バスへの依存を辺とする DAG を作り、トポロジカルソートで初期化順序を決める。
6. コード生成: `devicetree` モジュール（定数、型エイリアス）、`Devices` 構造体、`init()`、判別関数を出力する。
7. 補助的な出力
   - 標準の `dtc` で読める、合成済みの DTS と DTB（デバッグ用、将来の実行時方式用）
   - ピンの割り当て表

- `build.rs` から呼べるライブラリと、CLI（`kumiki-dt check | gen | pintable | dump`）の両方として提供する。
- リポジトリ内のボード定義から生成したコードは**コミットしておく**（esp-metadata-generated と同じ方式。レビューしやすく、docs.rs でも見える）。CI では、生成し直した結果に差分が出ないことを確かめる。ユーザーが自分のボードを書く場合は `build.rs` で生成する。

### 6.2 生成されるコード（単一のボードの場合）

```rust
// 生成されたコード（概略）
pub mod devicetree {
    pub mod nodes { pub mod lcd { pub const SPI_MAX_FREQUENCY: u32 = 40_000_000; pub const WIDTH: u16 = 320; /* … */ } }
    pub mod chosen { pub use super::nodes::lcd as display; /* … */ }
}

pub struct Devices {
    pub display: Display,              // 型エイリアス: kumiki_gfx::Display<DcsPanel<EspSpiDmaBus<'static>, Ili9342c>>
    pub touch:   Touch,
    pub imu:     Imu,
    pub rtc:     Rtc,
    pub power:   Power,
    pub buttons: Buttons,
    pub tasks:   BackgroundTasks,      // バックグラウンドで回す処理（ランタイムに依存しない Future。§7）
}
pub struct Resources { /* DMA バッファ、共有バスのセル（StaticCell） */ }

pub async fn init(p: Peripherals, r: &'static mut Resources, delay: impl DelayNs) -> Result<Devices, InitError> {
    // 依存グラフのトポロジカル順に、まっすぐ並んだコードとして生成される
    // i2c0 → pmic（aldo3, bldo1 を立ち上げる）→ ioexp → lcd（reset, power-supply）→ backlight → touch → imu → rtc
}
```

### 6.3 機種ファミリー（1 つのバイナリで複数のボードに対応する）

- ビルドの単位は「同じチップのボードの集合」。どのボードを含めるかは、Cargo の feature（`board-m5stack-cores3` など）で選ぶ。指定がなければ、そのチップのボードをすべて含める。
- 生成器は、各ボードの `kumiki,detect` と eFuse の情報から**判別の決定木**を作る。
  1. eFuse のパッケージ種別（と P4 の rev）で候補を絞る。
  2. 電源保持の GPIO を立てる。
  3. I2C の ID レジスタをプローブする。
  4. パネル ID（RDDID）を読む。
  - 「他のボードで出力として使われているピンを駆動しない」順序を制約として解く。M5GFX の `detector_order.json` に相当する。
- `Devices` の各フィールドは、ボードによって型が違えば `AnyDisplay` などの enum に、ボードによって存在しなければ `Option` にする。enum のトレイト実装は、match による委譲として生成する。
- ピンの型は `AnyPin` に揃える。match の腕ごとに、同じペリフェラルを別の用途で move できる。

### 6.4 共有バス、相互参照、glue ノード

- **I2C の共有**: 非同期のビルドでは、`embassy-embedded-hal` の `I2cDeviceWithConfig` と `embassy-sync` の `Mutex` を使う。どちらも executor に依存しない（§7）。デバイスノードの `clock-frequency` は、デバイスごとの設定に反映する。ブロッキングのビルドでは、`embedded-hal-bus` の `CriticalSectionDevice` を使う。
- **SPI の共有**: 表示が DMA で専有するのを基本にする。同じバスに SD カード（`mmc-spi-slot`）などがある場合は、共有のラッパーを生成する。SPI モードの異なるデバイスの共有には未解決の課題がある（embedded-hal #608）ので、モードはデバイスごとに持つ。
- **GPIO・割り込み・レギュレータの提供側と使用側**
  - `gpio-controller` を宣言したノード（AW9523、AXP192 の GPIO、PI4IOE5V6408）は、バインディングの `provides` で `OutputPin` / `InputPin` / `Wait` を提供する。
  - `*-gpios` で参照する側は、それを受け取る。
  - レギュレータの参照（`*-supply`）は、`Rail` トレイトで受け取る。
- **glue ノード**: DT で素直に書けない命令的な処理は、専用の compatible を持つノードとして記述し、そのバインディングが Rust の関数に結び付く。依存グラフに参加するので、実行される順序も保証される。
  - 例: CoreS3 の GPIO35（MISO と D/C の共用）、5V 昇圧の判断
  - 例: Tab5 の PI4IOE の「触ると電源が落ちる」ビットの保護（`kumiki,protected-lines = <4>;`）

## 7. 非同期ランタイムに依存しないこと

決定事項: async を既定にするが、**非同期ランタイムを embassy に固定しない**。

- ドライバと生成されるコードは、`embedded-hal-async` のトレイトだけを使う。待ち時間は、外から渡された `DelayNs` を使う。
- 同期の部品
  - **`embassy-sync`**（Mutex、Signal、Channel）は executor から独立した no_std のライブラリで、waker さえあれば任意の executor で動く。RTIC v2、esp-rtos、embassy-executor のどれでも使える。共有バス（`embassy-embedded-hal`）もこれに乗る。
  - 依存を減らしたい場合に備えて、`critical-section` だけで済む簡易な実装も用意する。
- 時刻: `embassy-time` は使わない（グローバルなタイマードライバを要求するため）。代わりに、最小限の `Clock` トレイト（`now() -> Instant`）を定義し、embassy-time、rtic-monotonics、esp-hal の `Instant` に対するアダプタを用意する。
- **タスクの起動（spawn）はライブラリの中で行わない。** バックグラウンドで回す処理（ボタンのポーリング、EPD の更新、オーディオのストリーミング）は、`Devices::tasks` が持つ `async fn run(&mut self)` の Future として返す。アプリは、自分の使うランタイムで spawn または join する。
  - 補助として、`embassy` feature で `#[embassy_executor::task]` のラッパーを、`rtic` feature で RTIC 向けの例を用意する。
- 割り込みと waker: esp-hal の async ドライバは、割り込みハンドラと `AtomicWaker` で実装されており、executor から独立している。P4 v1 向けの自前 HAL（§9）も同じ作りにする。
- ブロッキング版: 同じ DTS から、`init_blocking()` と、ブロッキングの共有バスを使う版を生成できるようにする（feature で選ぶ）。

## 8. オーバーレイ（Unit、Module、ユーザーによる追加）

- 外部コネクタ（Port A/B/C、Grove、M-Bus）は、Zephyr の Arduino ヘッダーと同じく nexus ノード（`gpio-map`）と、ラベルを付けたバス（`&port_a_i2c`）で記述する。
- Unit や Module は、`/plugin/` 形式のオーバーレイ（`.dtso`）として、コネクタのラベルにデバイスを追加する。
- `status = "fail-needs-probe"` を付けると、実行時にプローブして見つかった場合だけ `Option::Some` になる（取り外しのできる Unit 向け）。
- オーバーレイは Cargo の feature、`KUMIKI_DT_OVERLAYS` 環境変数、またはユーザーのクレートの `build.rs` で指定する。

## 9. Tab5（ESP32-P4 rev v1）と HAL アダプタ

- 決定事項: 手元の Tab5 は **rev v1** である。esp-hal は rev v3 以降にしか対応しない（[04](04-ecosystem-bsp.md) §1）ため、Tab5 では esp-hal を使えない。
- kumiki-dt は特定の HAL を前提にせず、HAL アダプタ（`kumiki-hal-*`）を通してコントローラを作る。このため、Tab5 用には別の HAL を差し込める。
  - **案 1**: コミュニティのフォーク `esp-hal-p4-pre-v3`（<https://github.com/naoNao89/esp32p4-pre-v3-kit>）を評価する。対象は rev 1.3 で、rev v1.0 で動くかどうかは確認していない。
  - **案 2（本命）**: tab5-os のレジスタ直叩き実装（I2C、GPIO、PSRAM、DSI、GDMA の LLI リング、PPA/DMA2D）を、embedded-hal のトレイトを実装した小さな HAL クレート（`kumiki-p4v1-hal`）として切り出す（[02](02-local-assets.md)）。
- **Tab5 は、esp-hal 以外の HAL でも kumiki-dt が成り立つことを確かめる最初の実例になる。**

## 10. 将来: 実行時に解析する方式（ACPI のような方式）

DTS を採用したことで、同じ記述を DTB にしてフラッシュのパーティションに置き、実行時に読む方式（`dtoolkit` や `fdt` で、alloc なしで解析できる）にも自然に拡張できる。ただし、ドライバはすべてリンクされ、型の検証は実行時になるので、v1 では作らない。

## 11. 汎用性の確認

- 生成器のテスト
  - ゴールデンファイル（生成されたコードのスナップショット）を比較する。
  - 不正な記述（循環、ピンの重複、未知の compatible、型の誤り）で生成が失敗することを確かめる。
  - 同じ DTS が標準の `dtc` でもコンパイルできることを CI で確かめる。
- M5 以外のボード（ESP32-S3-DevKitC + ST7789、RP2040 + ST7789）と、esp-hal 以外の HAL（Tab5 の P4 v1 用 HAL、embassy-rp）で、同じ仕組みが動くことを確かめる。
