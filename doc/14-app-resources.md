# 14. アプリに渡す資源と追加ボード

stackchan-rs（Stack-chan のファームウェア）を kumiki に移すときに、足りないものが 4 つ見つかった。

1. ボードが使わないピンや UART を、アプリが受け取れない（サーボの UART に要る）
2. ボードのデバイスが共有する I²C バスに、アプリがデバイスを足せない（台座の PY32 と INA226 に要る）
3. 音声を `await` で流せない（音声合成と再生を重ねるのに要る）
4. 台座のような追加ボードを DT に書けない

この文書は、それぞれをどう埋めたかをまとめる。1〜3 は実装済み（3 は実機で未確認）、4 は §4 の方針で進める。

## 1. 使わない資源（`Devices.free` / `Devices.spare`）

ボードは実行時に判別するので、どのピンが空いているかは、判別するまでわからない。そこで、空いている資源を 2 つに分けて渡す。

| 欄 | 中身 | 型 |
|---|---|---|
| `spare` | 族（ESP32-S3）のどのボードも使わないペリフェラル。タイマー、ソフトウェア割り込み、Wi-Fi、BT、RNG、2 つ目のコア、UART0〜2、SPI3、I2S0、DMA_CH2〜4、RMT、LEDC、PCNT、MCPWM、TWAI、ADC、LCD_CAM、SDHOST、暗号器 | 値そのもの |
| `free` | ボードによって使ったり使わなかったりするもの。すべての GPIO と、I2C0/1、SPI2、I2S1、DMA_CH0/1、PSRAM | `Option`。判別したボードが使うものは `None` |

- `free` の型（`Free`）と、各ボードでの中身は生成器が作る。
  - ピンの一覧は、SoC の `.dtsi` の `kumiki,io-pins`（HAL が持つピン）から `kumiki,spi-memory-pins`（SPI フラッシュと quad PSRAM が使うかもしれないピン）を除いたもの。どちらも esp-metadata から生成する。
  - ボードが使うペリフェラルは、生成した `init` の中の `p.X` から拾う。手で一覧を保つ必要はない。
- octal PSRAM を積んだチップでは、GPIO33〜37 を `None` にしてから返す（`Free::withhold_octal_psram_pins`）。
- DT に書いていないハードウェアにつながったピンも `Some` で返る。たとえば CoreS3 の Port A/B/C、カメラ、SD カード。何につながっているかは、アプリの側で確かめる。

例（CoreS3 + Stack-chan の台座のサーボ。TX=GPIO6、RX=GPIO7、1 Mbps）：

```rust
let uart = Uart::new(dev.spare.uart1, UartConfig::default().with_baudrate(1_000_000))?
    .with_tx(dev.free.gpio6.take().unwrap())
    .with_rx(dev.free.gpio7.take().unwrap());
// esp-hal の Uart は Send なので、2 つ目のコアのタスクへ渡せる
```

P4（Tab5）は `free` をまだ持たない。HAL の形が違うので、要る用途が出たときに入れる。

## 2. 共有の I²C バス（`Devices.i2c`）

`Devices.i2c: Option<&'static RefCell<I2cBus>>` は、ボードのデバイスが共有するバスを返す。

- どのバスを返すかは次の順に決める。
  1. DT の `chosen { kumiki,i2c = &i2c0; }` があれば、そのバス。
  2. なければ、ボードに有効なバスが 1 本だけのとき、そのバス。
  3. 2 本以上あって `chosen` がなければ、生成時にエラーにする。
- アプリのデバイスは `RefCellDevice::new(i2c)` で作る。
- `RefCell` なので 1 つのコアでしか使えない。ボードのデバイスを動かすコアから使う。

## 3. 非同期の音声（ESP32-S3）

kumiki-class に、同期版を親に持つ trait を 2 つ足した。

| trait | メソッド | 返るとき |
|---|---|---|
| `AsyncSpeaker: Speaker` | `play_async(&frames).await` | フレームが全部 DMA のリングに入ったとき |
| `AsyncMicrophone: Microphone` | `read_async(&mut out).await` | 1 フレーム以上たまったとき |

- 同じ `AnySpeaker` / `AnyMicrophone` が、同期版と非同期版の両方を持つ。変換は要らない。
- **待ち方**：I²S コントローラは esp-hal の async モードで作る（`I2s::new(..).into_async()`）。
  - 待つのは、リングの 1 区切り（descriptor）が終わったときの DMA の EOF 割り込み（esp-hal の `wait_for_available_async`）。表示と同じく実行器には依存せず、時間のドライバも要らない。
  - 同期版のメソッドは、これまでどおりポーリングで動く。
- **重ね方**：合成と再生を重ねるには、`join(synthesize(&mut next), spk.play_async(&cur))` のように future を並べる。
- **リングが空になったとき（再生）**：
  - 次の `play_async` までにリングが空になると、送信が止まる。
  - 止まったあとの最初の書き込みでは、リングを作り直す。古い中身を一度鳴らすことはしない。新しいフレームを先頭に詰め、足りなければ 2 区切りまで無音で埋めてから送り始める。これは同期版の `play` / `idle` も同じ。
- **注意**：送信が止まっている間は BCK / WS も止まり、同じコントローラのマイクが録れない。録音しながら鳴らす物がないときは、無音を流し続けるか `idle()` を呼ぶ。
- **リングがあふれたとき（録音）**：DMA が止まったことを割り込みで知り、すぐに録音を始め直す。同期版の 200 ms の監視を待たない。
- **確認用**：kumiki-async-demo の embassy 版に `audio_loop` を足した。`play_async` で 1 kHz を 0.2 秒鳴らして 0.8 秒休み、それを `read_async` で録る。1 秒ごとに、送ったフレーム数・録ったフレーム数・鳴っている間と休みの間の 1 kHz の強さを出す。
- P4（Tab5）はまだ持たない。kumiki-p4v1-hal の I²S に割り込みで起こす仕組みがないため。

## 4. 追加ボード（方針）

台座のように本体に重ねるボードは、ボードの変種として書く。

- `boards/dts/m5stack/cores3-stackchan.dts` は `cores3.dts` を `#include` し、次のノードを足す。
  - 内部 I²C の PY32（0x6F、`gpio-controller`）
  - INA226（0x41）
  - サーボの 5V（`regulator-fixed`。enable は PY32 の線 0）
  - サーボの UART（GPIO6/7）
- 判別では PY32 に問い合わせる。1 つのバイナリが、台座があれば「CoreS3 + Stack-chan」、なければ CoreS3 として動く。
- PY32 のドライバは `drivers/kumiki-py32` に置く。embedded-hal だけを使う。
