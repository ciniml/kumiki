# 05. 新規に作るか、既存クレートを拡張するか

[01](01-m5gfx-internals.md)〜[04](04-ecosystem-bsp.md) の調査結果をもとに、部品ごとに方針を決める。

## 判断の基準

- **上流に入れられる性質の変更か**（汎用で、上流の設計方針と合っているか）
- **上流の設計を変えずに目的を達成できるか**（性能の要である「送信開始後に待たない API」と「変換を DMA と重ねるパイプライン」を後付けできるか）
- **保守が続いているか、版の固定で他と組み合わせにくくならないか**

## 部品ごとの方針

| 部品 | 方針 | 理由 |
|---|---|---|
| 描画 API（図形、フォント、画像） | **embedded-graphics をそのまま使う** | 事実上の標準である。互換を保つことが要件 |
| 高速経路（矩形転送、生バッファの転送） | **新規**（拡張トレイト）。あわせて embedded-graphics に議論を持ちかける | 上流には生バッファを扱う経路と非同期版の DrawTarget がない。PR #747 は 2024 年から停滞している。先に自分たちのクレートで実証し、その結果を持って提案する |
| ピクセル形式 | **新規**（gfx-core）。将来は embedded-graphics の `DataOrder` と `Rgb332` に合わせる | 上流の Unreleased に入っている変更が出るまで待てない |
| パネルの初期化表（ILI9342C, GC9107, ST7789, ST7735, GC9A01, ILI9881C, ST7123） | **新規の Model 定義として移植**（出典を明記） | mipidsi の Interface はブロッキングで、送信開始後に待たない API を後付けしにくい（[03](03-ecosystem-graphics.md) §2）。初期化表は mipidsi（MIT）と M5GFX（MIT/BSD-2）の両方を参照する。DSI パネルは mipidsi に存在しない |
| mipidsi との互換 | **アダプタを提供する**（任意） | mipidsi の `Interface` を自前のバスで実装すれば、mipidsi の利用者もこちらのバスで速くなる。mipidsi 本体には手を入れない |
| バス抽象と ESP 用の DMA パイプライン | **新規** | 既存の高レベル実装がない。esp-lcd-i8080 は考え方の先例として参照する |
| esp-hal の DMA / LCD_CAM / I2S / DSI | **上流に貢献する** | バグ修正、P4 の PPA/DMA2D ドライバ（rev ≥ v3）、`DmaTxBuf` の改善などは上流に入れるべきもの。本プロジェクトは esp-hal の上に乗る |
| ESP32-P4 rev v1.x の DSI / GDMA / PPA | **新規**（tab5-os の実装を移植） | 上流は v1.x に対応しない方針（[04](04-ecosystem-bsp.md) §1） |
| スプライト、回転・拡縮、パレット | **新規** | 既存実装が分散しており、DMA と連携するものがない |
| Slint との接続 | **新規のアダプタ**。Slint の mcu-board-support（CoreS3）には DMA 化の改善を貢献 | Slint はライセンスの都合で依存の必須にはしない（[03](03-ecosystem-graphics.md) §5） |
| 電源 IC（AXP192/AXP2101）、FT6336U、BM8563 | **既存を使う**: okhsunrog 氏の `*-dd` 系 | eh1 と async の両対応で、2026-09 に更新されている（[04](04-ecosystem-bsp.md) §3） |
| AW9523B | **既存を使う**: `embedded-aw9523` | eh1 と async の両対応 |
| GT911 | **既存を使う**: `gt911`。不足している機能（INT 線による起動シーケンスなど）は貢献する | 最も多く使われている |
| BMI270 | `bmi2` を使い、async 対応を貢献する。だめならフォークする | ブロッキングのみのため |
| ES8311 / ES7210 | 評価したうえで使うか貢献する。品質が足りなければ新規に書く | どちらも出たばかりで若い |
| MPU6886、BMM150、AW88298、ES8388、IP5306、PI4IOE5V6408、RX8130CE、LTR-553、ST7123 のタッチ | **新規**（独立したクレートとして公開し、汎用に使えるようにする） | 公開されたクレートがない |
| PaperS3 の EPD | **新規**（寛容なライセンスでクリーンルーム実装） | 既存の実装はどれも GPL-3.0（[04](04-ecosystem-bsp.md) §8） |
| ボタン | **新規**（ビットマスクに対する共通のデバウンス処理）。内部ロジックは `debouncr` 相当 | ボタンの入力源が GPIO、PMIC、エキスパンダ、タッチ領域と雑多なため |
| バスの共有 | **既存を使う**: `embedded-hal-bus`、`embassy-embedded-hal`（`I2cDeviceWithConfig`） | 十分に使える |
| ハードウェア記述層 / BSP | **新規**（本プロジェクトの中核） | 既存の M5 向けクレートはどれも 1 機種専用で、esp-hal の版も厳密に固定されている。標準となる記述方法がない（[07](07-design-hwdesc.md)） |
| M5GFX / LovyanGFX の FFI バインディング | **採用しない** | std 前提であり、DrawTarget との境界で性能を失う（[02](02-local-assets.md)） |

## stackchan-rs / m5drivers-rs の扱い

- m5drivers-rs の AXP2101、AXP192、AW9523B、ILI9342 の DMA 版は、本プロジェクトの最初の実装の下敷きにする（ライセンスは MIT OR Apache-2.0 で、同じ作者圏）。
- ドライバ単体は、上の表の既存クレートと機能や品質を比べ、どちらか一方に寄せる。
- 本プロジェクトが形になったら、stackchan-rs の `boards/m5stack` を本プロジェクトの BSP に置き換え、実際のアプリでの検証に使う。

## 上流への貢献の候補（優先順）

1. esp-hal: SPI DMA まわり（`write_buffer` の連続実行時のオーバーヘッド、チャンク長の上限）、LCD_CAM の不具合、P4 の PPA/DMA2D。
2. embedded-graphics: 連続したバッファを渡せる `fill_contiguous` 相当の経路（`ImageRaw` を高速に描く道筋）、非同期版の DrawTarget の議論への実測データの提供。
3. 既存ドライバ（gt911, bmi2, es7210 など）の async 対応と M5 での実機検証。
4. Slint: CoreS3 のボードサポートの DMA 化。
5. ハードウェア記述
   - Ariel OS の sbd は、YAML のボード記述から Rust コードを生成する仕組みで、「他プロジェクトでも使えること」を目標に掲げている。ただし今のところデバイス（バス上の外付けチップ）の記述がない（<https://github.com/ariel-os/sbd>）。HWD のデータモデルを固めた段階で、相互に変換できるようにするか合流するかを相談する。
   - M5GFX の board_spec は、本人たちも「試作段階」と書いている（<https://github.com/m5stack/M5GFX/tree/master/extras/board_spec>）。不足している情報を提供するなど、データを共有する方向で協力する。
