# 14. アプリに渡す資源と追加ボード

stackchan-rs（Stack-chan のファームウェア）を kumiki に移すときに、足りないものが 4 つ見つかった。

1. ボードが使わないピンや UART を、アプリが受け取れない（サーボの UART に要る）
2. ボードのデバイスが共有する I²C バスに、アプリがデバイスを足せない（台座の PY32 と INA226 に要る）
3. 音声を `await` で流せない（音声合成と再生を重ねるのに要る）
4. 台座のような追加ボードを DT に書けない

この文書は、それぞれをどう埋めたかをまとめる。4 つとも実装済みだが、3 と 4 は実機でまだ確かめていない。

## 1. 使わない資源（`Devices.free` / `Devices.spare`）

ボードは実行時に判別するので、どのピンが空いているかは、判別するまでわからない。そこで、空いている資源を 2 つに分けて渡す。

| 欄 | 中身 | 型 |
|---|---|---|
| `spare` | 族（ESP32-S3）のどのボードも使わないペリフェラル。タイマー、ソフトウェア割り込み、Wi-Fi、BT、RNG、2 つ目のコア、UART0/2、SPI3、I2S0、DMA_CH2〜4、RMT、LEDC、PCNT、MCPWM、TWAI、ADC、LCD_CAM、SDHOST、暗号器 | 値そのもの |
| `free` | ボードによって使ったり使わなかったりするもの。すべての GPIO と、I2C0/1、SPI2、I2S1、UART1、DMA_CH0/1、PSRAM | `Option`。判別したボードが使うものは `None` |

- どちらの一覧も生成器が作る。ボードを足して使うペリフェラルが変わると、`spare` から `free` へ自動で移る。

- `free` の型（`Free`）と、各ボードでの中身は生成器が作る。
  - ピンの一覧は、SoC の `.dtsi` の `kumiki,io-pins`（HAL が持つピン）から `kumiki,spi-memory-pins`（SPI フラッシュと quad PSRAM が使うかもしれないピン）を除いたもの。どちらも esp-metadata から生成する。
  - ボードが使うペリフェラルは、生成した `init` の中の `p.X` から拾う。手で一覧を保つ必要はない。
- octal PSRAM を積んだチップでは、GPIO33〜37 を `None` にしてから返す（`Free::withhold_octal_psram_pins`）。
- DT に書いていないハードウェアにつながったピンも `Some` で返る。たとえば CoreS3 の Port A/B/C、カメラ、SD カード。何につながっているかは、アプリの側で確かめる。

例（素の CoreS3 の Port C に UART をつなぐ。RX=GPIO18、TX=GPIO17）：

```rust
let uart = Uart::new(dev.spare.uart2, UartConfig::default().with_baudrate(115_200))?
    .with_rx(dev.free.gpio18.take().unwrap())
    .with_tx(dev.free.gpio17.take().unwrap());
// esp-hal の Uart は Send なので、2 つ目のコアのタスクへ渡せる
```

Stack-chan の台座のサーボの UART は、DT に書いたので `Devices.extras` から受け取る（§4）。

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

## 4. 追加ボード（Stack-chan の台座）

台座のように本体に重ねるボードは、**ボードの変種**として書く。

- CoreS3 の中身は `cores3.dtsi` に移した。`cores3.dts` と `cores3-stackchan.dts` の両方がこれを `#include` する。
- `cores3-stackchan.dts` は、次のものを足す。

| ノード | 中身 | 生成されるもの |
|---|---|---|
| `base_io: gpio@6f` | 内部 I²C の PY32（`m5stack,py32-io`）。`kumiki,leds = <12>` | 線 0 と 13 を出力に設定し、LED を消す。`kumiki_py32::Py32`（LED 用） |
| `servo_power` | `regulator-fixed`。enable は PY32 の線 0、`startup-delay-us = <200000>` | `kumiki_class::Rail<GpioSwitch<Py32Pin, Delay>>`。立ち上げ直後は切ってある |
| `battery: power-monitor@41` | INA226（`ti,ina226`、`shunt-resistor = <10000>` µΩ） | `Option<kumiki_ina226::Ina226>` |
| `&uart1` | GPIO6/7、`current-speed = <1000000>` | `esp_hal::uart::Uart<'static, Blocking>` |
| `yaw: servo@1`, `pitch: servo@2` | `feetech,scs0009`。`reg` は ID、`kumiki,zero`、`kumiki,range` | 定数（`yaw::ID`、`yaw::ZERO`、`yaw::RANGE`）。サーボの通信の手順はアプリが持つ |

- **アプリへの渡し方**：`chosen { kumiki,extras = <&base_io>, <&servo_power>, <&battery>, <&uart1>; }` に並べたノードが、`Devices.extras` に入る。
  - `Devices.extras` は族の enum（`Extras::None` / `Extras::M5StackCoreS3Stackchan(cores3_stackchan::Extras)`）。
  - 欄の名前はノードのラベル、型はバインディングの `type` から決まる。
  - `type` の中では `${outpin-type:gpios}` を使える。線の型は、線を持つ側のバインディングの `gpio-pin-type` から引く。
- **判別**：変種は CoreS3 より先に調べる。
  - CoreS3 の 2 つの問い合わせに加えて、PY32 のバージョン（レジスタ 0x02）を読む。0x00 / 0xFF は起動中とみなす（`kumiki,reject`）。
  - PY32 は起動が遅いので、電源投入から 1.2 秒たつまで 50 ms ごとに聞き直す（`kumiki,boot-time-ms`）。
  - その代わり、台座のない CoreS3 は電源投入のときに 1.2 秒待つ。気になるなら `board-m5stack-cores3` の feature で候補を絞る。
- **Spare と Free も生成する**：どのボードも使わないペリフェラルが `Spare` に入る。変種が UART1 を使うので、UART1 は `Spare` から `Free` に移った（素の CoreS3 では `Some`）。
  - 一覧の元は、生成器の `ESP32S3_APP_PERIPHERALS` に持つ。
- **ドライバ**：`drivers/kumiki-py32` と `drivers/kumiki-ina226`。どちらも embedded-hal だけを使い、テストを持つ。
  - INA226 の電流の範囲は、シャントの全幅（81.92 mV）に合わせた。そうすると較正値はいつも 2048 になり、1 LSB は 2.5 µV ÷ R になる。10 mΩ なら 0.25 mA。
- 使い方の例：

```rust
if let Extras::M5StackCoreS3Stackchan(mut base) = dev.extras {
    base.servo_power.enable()?;            // VM_EN を上げて 200 ms 待つ
    let mv = base.battery.as_mut().map(|b| b.bus_mv());
    base.base_io.set_leds(0, &[kumiki_py32::rgb565(0, 32, 0); 12])?;
    base.base_io.show_leds()?;
    let servo_uart = base.uart1;           // 2 つ目のコアのタスクへ
}
```
