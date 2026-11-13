# 15. バッファとメモリ（Allocator API と PSRAM）

## 背景

kumiki を使うアプリから、メモリについての要望が 2 つ届いた。

- **simple_matter**（Wi-Fi + BLE + Matter）：内蔵 DRAM が 15,008 B 足りず、リンクに失敗した。
  - `init(p)` が抱える表示用の DMA バッファ（16 KB × 2）に加えて、有効な全ボードの `init` を参照するので、CoreS3 の I²S のリング（約 64 KB）までリンクされる。
- **stackchan-rs**：I²S のリングが、ボードの変種ごとに static で持たれる。

どちらも、kumiki がバッファを static で抱え込み、アプリから大きさも置き場所も選べないことが原因になっている。

Rust 1.100 で Allocator API（`core::alloc::Allocator`、`Vec::new_in` など）が正式になった。esp のツールチェーンは nightly 扱いなので 1.95 でも `#![feature(allocator_api)]` で使える。esp-alloc 0.11 は、メモリの種類ごとのアロケータ（`InternalMemory`、`ExternalMemory`、`DmaCompatibleInternalMemory`）を持っている。

## 方針

1. **alloc なしで使えることは変えない**（doc/09 §3）。バッファは呼び出し側が `&'static mut` で渡せる。
2. **alloc を使うのは、起動時にバッファを確保するところだけ**にする。確保したものは手放さない（`'static` にする）。
3. **alloc に依存するコードは別のクレート（`kumiki-boards-alloc`）に閉じ込める**。kumiki-gfx、kumiki-class、ドライバ、kumiki-boards は alloc に依存しない。
   - doc/09 の「kumiki-gfx の `alloc` feature で Vec ベースのスプライト」は要らない。`Sprite<F, B>` と `BufferedDisplay<D, F, B>` は `B: AsRef<[u8]> + AsMut<[u8]>` を受け取るので、`Vec<u8, ExternalMemory>` をそのまま渡せる。
4. **アロケータの trait は allocator-api2 のものを使う**。esp-alloc が実装していて、esp 1.95 でも stable の P4 でも feature の指定なしで動く。esp のツールチェーンを 1.100 以降に上げられたら、`core::alloc::Allocator` に移る（allocator-api2 は nightly では core の trait をそのまま使う）。
5. **PSRAM の使い方（モード）は DTS で決める**。

## 今のバッファ

| バッファ | 今 | 変えたあと |
|---|---|---|
| 表示の DMA バッファ（2 本） | `init` の中の `dma_tx_buffer!(16384)` × 2 | 呼び出し側から `Buffers` で渡す。大きさの目安はバインディングから生成 |
| I²S のリング（送信・受信） | バインディングのテンプレートの `dma_*_stream_buffer!`。変種ごとに static | 同上 |
| StopWatch のフレームバッファ | `psram_frame_buffer` が PSRAM 全体を割り当て、先頭を切り出す。残りは使えない | PSRAM の DT ノードから切り出し、残りを `Devices.psram` で返す |
| Tab5 の DSI フレームバッファ | PSRAM の最上部、1 MiB 境界に固定 | 変えない（キャッシュの先読みの境界と合わせている） |
| アプリのスプライト | `B` を呼び出し側が渡す | 変えない |

## 1. バッファを外から渡す（kumiki-boards、alloc なし）

### 必要量の生成

バッファを使うバインディングは、`kumiki.buffers` に名前と既定の大きさを書く。

```yaml
- compatible: "awinic,aw88298"
  kumiki:
    buffers: { i2s-tx: [16000, 4000] }   # (長さ, 1 区切りの大きさ)
    init: |
      ...
      let out = audio::I2sOut::new(tx, ${buffer:i2s-tx}, ...)
```

- `${buffer:NAME}` は、呼び出し側が渡した `Buffers` から取り出す式になる。渡されていなければ `InitError::MissingBuffer("NAME")` で失敗する。
- バッファの種類は 3 つ。`display`（DMA の送信バッファ 2 本）、`i2s-tx`（送信のリング）、`i2s-rx`（受信のリング）。
- 生成器は、ボードごとに `BUFFERS: BufferNeeds`（そのボードの有効なノードが使うもの）を出す。族には `buffer_needs(BoardId)` と、検出の候補（`board-*` feature で絞ったもの）の中で一番大きい `MAX_BUFFERS` を出す。

### API（ESP32-S3）

```rust
// いちばん手軽：今までどおり
let dev = kumiki_boards::init(p)?;

// バッファを自分で用意する
let board = kumiki_boards::detect(&mut p)?;            // Peripherals は借用だけ
let need = kumiki_boards::buffer_needs(board);         // 大きさの目安
let bufs = Buffers { display: Some((a, b)), i2s_tx: None, i2s_rx: None };
let dev = kumiki_boards::init_with(p, board, bufs)?;
```

- `init(p)` は、`MAX_BUFFERS` の大きさの static を **1 組だけ**持つ（ボードの変種ごとには持たない）。判別したボードが使うものだけを渡す。
- `init_with` に渡すバッファは、目安より小さくてもよい（表示は 2 KiB × 2 でも動く。速さは落ちる）。I²S のリングは、descriptor が 4 本以上必要。
- 判別したボードが使わないバッファは、渡しても使われない。

### 候補でないボードのコードをリンクしない

`init_board` の各分岐を `if <board>::CANDIDATE` で守る。`board-*` feature で候補を絞ると、ほかのボードの `init` はリンクされない。候補でないボードを `init_with` に渡すと `InitError::NotCandidate` になる。

## 2. PSRAM を DTS で決める

- SoC の `.dtsi` に `psram` ノードを足す（esp-metadata から生成。既定は `status = "disabled"`）。
- PSRAM を積んだボードは、ボードの DTS で有効にし、モードを書く。

```dts
&psram {
	status = "okay";
	kumiki,mode = "octal";   /* "quad" | "octal" */
};
```

- 生成した `init` は、そのモードで PSRAM を割り当てる（esp-hal の `PsramMode::Auto` で両方を試すことはしない）。
- PSRAM にフレームバッファを置く表示は、`memory-region = <&psram>` で PSRAM ノードを指す（標準のプロパティ）。生成器はこれを依存関係として扱い、PSRAM を先に割り当てる。
- 表示などが切り出した残りは `Devices.psram: Option<&'static mut [u8]>` でアプリに渡す。
- DT で PSRAM を有効にしていないボードでは、`Devices.free.psram`（ペリフェラルそのもの）が `Some` のまま残る。アプリが自分で設定できる。
- 書いたボード：CoreS3（とその変種）は `quad`、StopWatch は `octal`。AtomS3 は PSRAM を持たない。
- **測った結果**（kumiki-demo、`board-*` で 1 ボードに絞ったビルド）：
  - **モードを決めても、もう一方のドライバは IRAM から消えない**。StopWatch の `.rwtext` は 9,032 B で変わらなかった。esp-hal 1.2 は、モードを実行時の値として 1 つの IRAM の関数の中で振り分けるので、使わない側もリンクされる。消すには esp-hal 側の変更（モードを型や feature で選ぶ）が要る。DT で決める利点は、`Auto` の判別（octal を試してから quad）をしないことにとどまる。
  - CoreS3 は、kumiki が PSRAM を割り当てるようになったので、`.rwtext` が 5,568 → 9,032 B に増えた（PSRAM の初期化コードは IRAM に置かれる）。PSRAM を使うアプリなら、もともと払っていた分。
- **CoreS3 の変更点**：これまで `Devices.free.psram` で受け取っていた PSRAM は、`Devices.psram`（割り当て済みの領域）で受け取る。`esp_alloc::psram_allocator!` を使っていたアプリは、`kumiki_boards_alloc::add_psram_to_heap(dev.psram.take().unwrap())` などに置き換える（§3）。

## 3. kumiki-boards-alloc（alloc に依存するのはここだけ）

```rust
// esp-alloc の DMA 可能な内蔵メモリから、判別したボードが使う分だけ確保する
let dev = kumiki_boards_alloc::init_in(p, esp_alloc::DmaCompatibleInternalMemory)?;
// PSRAM の残りを esp-alloc のヒープに足す（feature `esp-alloc`）
if let Some(rest) = dev.psram.take() { kumiki_boards_alloc::add_psram_to_heap(rest); }
```

- `init_in<A: Allocator + 'static>(p, dma: A)`：`detect` → `buffer_needs` → 確保 → `init_with`。確保したバッファは手放さない。
- バッファと descriptor は、esp-hal の DMA が届く内蔵 RAM に置く必要がある。どのアロケータを渡すかは呼び出し側が決め、esp-hal の `DmaAlignedMut::new` で置き場所と境界を確かめる。
- `add_psram_to_heap`：PSRAM の残りを `esp_alloc::HEAP` に `External` として足す。以後 `Vec<u8, ExternalMemory>` が使える。
  - **注意（ESP32-S3）**：PSRAM 上では atomic 命令が正しく動かない（esp-alloc の `psram_allocator!` と同じ制約）。グローバルアロケータが PSRAM から確保した領域に `Atomic*`（`Arc` の中身など）が入らないようにする。大きなバッファは `ExternalMemory` を明示して確保するのが安全。
- `init_from_heap(p)`（feature `esp-alloc`）は `init_in(p, &esp_alloc::DmaCompatibleInternalMemory)` の短縮形。
- 依存：kumiki-boards、esp-hal、allocator-api2 0.3（esp-alloc 0.11 と同じ版）、esp-alloc（任意）。ルートの workspace からは外し、firmware/ のビルドで使う（kumiki-boards と同じ）。
- **確認用**：kumiki-async-demo の embassy 版に feature `alloc` を足した（`cargo build --release -p kumiki-async-demo --features alloc --bin embassy`）。72 KB のヒープからバッファを取り、PSRAM の残りをヒープに足して、ヒープの統計を表示する。
  - `.bss` は static 版より 8.5 KB 多い（234,912 → 243,496 B）。ヒープを固定の 72 KB にしているためで、CoreS3 のバッファ（約 65 KB）を引いた残りはアプリが使える。AtomS3 ならバッファは 33 KB で、残りはもっと多い。

## 4. P4（Tab5）

- DSI のフレームバッファは今の固定の置き場所のままにする。
- その下の PSRAM の範囲を `Devices.psram` で返すのは、P4 のアプリで用途が出たときに足す（kumiki-p4v1-hal にはヒープがない）。

## 進め方

1. `kumiki.buffers` / `${buffer:…}`、`BufferNeeds`、`detect` / `init_with`、static の 1 組化、候補でないボードの除外。**済**（AtomS3 だけのビルドで `.bss` 97,952 → 33,208 B、`.rwtext` 9,192 → 5,568 B。全ボードで `.bss` 97,952 → 65,688 B）
2. PSRAM ノードと `kumiki,mode`、`memory-region`、`Devices.psram`。**済**
3. kumiki-boards-alloc。**済**
4. デモでの使用例。**済**（kumiki-async-demo の feature `alloc`）

どれもビルドとテストまで。実機ではまだ確かめていない（PSRAM のモード指定、ヒープから取ったバッファでの表示と音声）。
