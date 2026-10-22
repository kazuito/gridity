# gridity 実装プラン

PDF・動画・画像・GIF/APNG の指定範囲をラベル付きグリッド画像に並べ、LLM が 1 回の画像読み込みで複数ページ/コマを把握できるようにする CLI。

## 0. 決定事項

| 項目 | 決定 |
|---|---|
| PDF 描画 | **hayro**（純 Rust、外部ライブラリ不要で単一バイナリ） |
| 動画 | **ffmpeg / ffprobe をサブプロセス呼び出し**（FFI バインディングは使わない） |
| GIF/APNG | **`image` crate で直接デコード**（`AnimationDecoder` が GIF の disposal / APNG の合成済みフレームを返すことを確認済み）。ffmpeg 不要 |
| 配布 | 当面は個人利用（macOS）。配布 CI・ライセンス整理は後回し |
| 既定サイズ | **互換重視（標準 tier）**: 長辺 1568px 以内 かつ `⌈w/28⌉×⌈h/28⌉ ≤ 1568`（Claude Vision 公式ドキュメントで確認済み） |
| v1 入力 | PDF / 動画 / 画像ファイル群 / GIF・APNG |
| 既定出力先 | **`$TMPDIR/gridity/<stem>-<hash>/`**。stdout に絶対パスを出力して LLM に伝える。`-o` で上書き可 |
| preset 指定 | **`--preset` 引数のみ**（環境変数は v1 では採用しない。§10 参照） |
| ガイド | **`gridity guide [topic]`** で LLM が自主的に参照できるドキュメントをバイナリに内蔵（§6） |

### Rust を深く使わないための方針

- 関数間は所有型（`Vec<u8>` / `RgbaImage` / `String`）で受け渡し、ライフタイム注釈を書かない
- 入力ソースは `enum` 1 つで分岐。ジェネリクスや複雑な trait は使わない
- async なし。並列化は `rayon` の `par_iter` のみ
- エラーは `anyhow::Result` + `.context()`
- 計算部分（レイアウト、範囲パース）は純粋関数にしてユニットテストで固める

---

## 1. CLI 仕様

```
gridity render <INPUT>... [OPTIONS]   # 種別は自動判定（--type で上書き）
gridity probe  <INPUT>    [--json]    # ページ数・長さ・解像度などを表示（PDF は --per-page）
gridity text   <PDF>      [--json]    # 見えているテキストとページごとの内容種別
gridity guide  [TOPIC]                # エージェント向け使い方ドキュメント（Markdown）
```

- 種別判定は拡張子とマジックバイト。複数入力は画像のみ可（PDF と動画の混在はエラー）
- **入力に合わないオプションはエラー**（例: 動画に `--pages` → exit 2）。黙って無視しない

### 範囲指定

| 入力 | オプション | 既定 |
|---|---|---|
| PDF | `--pages 1-3,5,10-`（1 始まり、`-5` = 1〜5） | 全ページ |
| 動画・GIF | `--start` / `--end`、`--every 5s` / `--count 24` / `--at 1:00,1:30` | 範囲全体から、シート 1 枚に収まる数を均等に取る |
| 画像 | 引数でファイル / ディレクトリ（自然順ソート） | 全部 |
| すべて | `--crop X,Y,W,H`（表示サイズに対する割合。回転・EXIF・SAR 適用後） | 全体 |

- 時刻は `90`、`90s`、`1:30`、`00:01:30.5`、`1m30s` を受け付ける
- `--count` は各区間の**中点**を取る（冒頭の黒画面を避けるため）

### オプション（v1 / later）

| 区分 | オプション | 時期 |
|---|---|---|
| 出力 | `-o/--output <FILE\|DIR>`、`--format png\|jpeg`、`--quality` | v1 |
| サイズ | `--preset compat\|hires`、`--max-edge`、`--max-tokens` | v1 |
| 密度 | `--detail low\|medium\|high`、`--per-sheet N`、`--cols` / `--rows`、`--max-sheets N` | v1 |
| 見た目 | `--gap`、`--padding`、`--bg <#hex>` | v1 |
| ヘッダー | `--title <TEXT>`、`--no-header` | v1 |
| ラベル | `--label <TEMPLATE>`、`--no-label`、`--label-pos below\|overlay`、`--label-size`、`--label-color`、`--label-bg` | v1 |
| フォント | `--font <PATH>` | v1 |
| エージェント向け | `--json`、`--dry-run`、`-q` / `-v`、`-j/--jobs` | v1 |
| 外部ツール | `--ffmpeg <PATH>`（または `GRIDITY_FFMPEG`） | v1 |
| PDF | `--password`（hayro がパスワード付き PDF に対応していたため v1 に前倒し） | v1 |
| 後回し | `--fit cover`、`--cell WxH`、`--border`、`--scene`（シーン検出）、設定ファイル、WebP 出力、アニメーション WebP、`gridity clean` | later |

- ラベルのテンプレート変数: `{page}` `{total}` `{time}` `{index}` `{name}` `{frame}`
- 既定ラベル: PDF は `p.{page}`、動画・GIF は `{time}`、画像は `{name}`
- 時刻表記: 1 時間未満は `m:ss`、1 時間以上は `h:mm:ss`、間隔が 1 秒未満なら小数 1 桁
- ラベル位置の既定: PDF・画像は `below`（セル下の帯）、動画は `overlay`（左上の半透明バッジ）
- ヘッダーは既定で表示（例: `report.pdf · p.1–4 / 42 · sheet 1/11`）。複数シートのとき LLM が現在位置を把握できるようにするため

---

## 2. 自動レイアウト

**制約**: Claude と同じ判定式 `fits(w, h)` を実装し、シート全体（ヘッダー・余白・ラベル込み）がその範囲に収まるようにする。

| preset | 長辺上限 | トークン上限 |
|---|---|---|
| `compat`（既定） | 1568px | 1568 |
| `hires` | 2576px | 4784 |

上限ちょうどに作るので API 側で縮小されない。そのため manifest に書くセル座標が、LLM が見る画像の座標と一致する。

**アルゴリズム**（`layout.rs`、純粋関数）

1. 枚数 N・セル縦横比・制約を受け取る。各 `cols` について `rows = ⌈N/cols⌉` とし、制約内で最大のセルサイズを二分探索で求め、セル面積が最大の組を選ぶ
2. セルが `--detail` の最小サイズを下回らない範囲で、1 シートあたりの最大枚数を決める
3. **均等に分割する**（最大 8 枚で 10 ページなら 8+2 ではなく 5+5）
4. 必要なシート数が `--max-sheets`（既定 10）を超えたらエラー。メッセージに「`--pages` で絞る / `--detail low` にする / `--max-sheets 11` にする」など具体的な対処を含める
5. ページサイズが混在する PDF は、セル比率に中央値を使い、各ページはその中にレターボックスで収める（contain）

**試算**（compat、余白 12px・隙間 8px・ヘッダー 32px・ラベル 22px、セルの px）

| 入力 | 4 枚/シート | 9 枚 | 12 枚 | 16 枚 |
|---|---|---|---|---|
| A4 縦 PDF | 432×611 (2×2) | 285×403 | 247×349 | 212×300 |
| 16:9 動画 | 689×388 | 455×256 | 402×226 | 338×190 |
| 4:3 画像 | 586×440 | 386×290 | 332×249 | 282×212 |

参考: hires では A4 が 4 枚で 782×1106。

**`--detail` の既定（1 シートあたりの目安枚数）**

| 入力 | low | medium（既定） | high |
|---|---|---|---|
| PDF | 9 | **4** | 2 |
| 動画 | 16 | **12** | 6 |
| 画像 | 12 | **6** | 3 |

互換重視のトレードオフ: PDF の本文は高さ 611px 程度だとかろうじて読める水準。図表の把握は問題ない。細部が必要なときは、エージェントが `--pages 7 --detail high` のように再実行して拡大する。

---

## 3. ソースごとの実装

### PDF（`source/pdf.rs`）

- hayro は **0.x の正確なバージョンに固定**
- hayro の借用型は `fn render_page(bytes: &[u8], index: usize, target_px: (u32, u32)) -> Result<RgbaImage>` の中に閉じ込める
- 描画倍率は「セル px ÷ ページ pt」で計算し、セルサイズで直接描画する（縮小の無駄を出さない）
- 背景は不透明（白）で描画し、アルファの扱いを不要にする
- ページの回転は `render_dimensions()` で考慮
- 暗号化 PDF はエラー（exit 3）で `--password` を案内する
- 並列化は hayro の型が `Send` / `Sync` か確認してから。まずは逐次処理
- `--crop` は `hayro::render_into` に平行移動を含む変換を渡し、切り抜き範囲だけをセルサイズで
  ラスタライズする（ページ全体を描いてから切ると、小さい範囲ほどメモリが膨らむため）

### ページ解析（`pagetext.rs` と `source/pdf.rs::scan_page`）

エージェントが「テキストで読めるページ」と「画像で見るべきページ」を分け、読める文字サイズの
詳細度を選べるようにする。

- `scan_page` は hayro の `Device` を実装した `Collector` でページを解釈だけして、見えている
  グリフ（位置・サイズ・Unicode）、画像と図形の範囲を集める。クリップは外接矩形で追跡し、
  クリップ外やページ外のグリフは数えない（スライドに切り抜いて貼った教科書ページなど）
- 集計と判定は純粋関数（`pagetext.rs`）: 本文サイズ（文字数で重み付けした中央値）、小さい
  文字（10 パーセンタイル）、画像・図形の被覆率（48x48 の格子で重なりを一度だけ数え、
  ページ全面の背景とページの半分以上を囲む枠線は除く）、数式の兆候（上付き・下付き、
  1〜2 文字の行の割合）
- 内容種別: `text` / `mixed` / `graphics` / `empty`。閾値は `pagetext.rs` の定数
- 読みやすさの目安は 8 px（`READABLE_PX`）。11pt 本文の report.pdf を各詳細度で目視し、
  6.5 px はかろうじて読める、8 px は読めると判断した。`probe` は本文がこれ以上になる最小の
  詳細度を提案するだけで、`render` の既定（`min_cell`）は変えない

### 動画（`source/video.rs`）

- 情報取得は `ffprobe -of json -show_streams -show_format`。長さは format と stream の両方を見る
- **レイアウトには表示上の寸法を使う**: 縦横比は `w × SAR / h`、回転メタデータが 90° / 270° なら幅と高さを入れ替える。ffmpeg はデコード時に自動回転するので、エンコード上の寸法を使うとレイアウトが崩れる
- 1 フレームの取得:
  `ffmpeg -ss T -i IN -frames:v 1 -vf "scale=…:force_original_aspect_ratio=decrease,setsar=1" -f image2pipe -c:v png -`
  を実行し、stdout を `image` でデコードする（一時ファイルなし）
- 複数のタイムスタンプは rayon で並列実行（同時数は `-j`、既定は min(コア数, 8)）
- 終端付近でフレームが返らない場合は少し手前で取り直し、それでも駄目なら警告を出してスキップ
- ffmpeg が見つからない場合は exit 4。`brew install ffmpeg` と `GRIDITY_FFMPEG` を案内する

### GIF/APNG（`source/anim.rs`）

- `image` の `AnimationDecoder` で合成済みフレームを順に読み、delay を積算して時刻を求める
- 動画と同じ時刻ベースの選択（`--every` / `--count` / `--at`）を共有する

### 画像（`source/images.rs`）

- デコードせずに寸法と EXIF Orientation だけを先に読み、レイアウトを確定させる
- 描画時にデコードして `apply_orientation` を適用する（スマホの JPEG が横倒しにならないように）

### メモリ

シート単位で処理する。保持するのは処理中のシートのセルだけ。

---

## 4. 描画とフォント

- 合成は `image` + `imageproc`（`draw_text_mut`）、文字は `ab_glyph`
- **3 つのバージョンを揃える**: `cargo tree` で確認し、可能なら imageproc が再エクスポートしている型を使う
- フォントは**日本語を含む OFL フォントを 1 つ埋め込む**（候補: BIZ UDGothic。小さい文字でも読みやすい）。サイズとライセンスは実装時に確認する
- 文字ごとのフォールバックはしない。`--font` で差し替えられるようにするだけ

---

## 5. 出力の約束（LLM ファースト）

### 出力先

- 既定は `$TMPDIR/gridity/<stem>-<hash>/`（`std::env::temp_dir()`）
  - `<hash>` は「入力の絶対パス + 描画オプション」から作る短いハッシュ。同じ条件で再実行すると同じディレクトリに上書きされる（入力ファイルを編集した後の再実行も同じディレクトリに上書きされ、tmp に古いディレクトリが溜まらない）
  - 再実行でシート数が減った場合は、そのディレクトリに残った古いシートを削除する（gridity が管理するディレクトリに限る）
- `-o` を指定した場合は、そこにファイルまたはディレクトリとして出力する
- ファイル名: 1 枚なら `<stem>.grid.png`、複数枚なら `<stem>.grid-01.png` のような連番

### stdout / stderr

- **stdout にはシートの絶対パスを 1 行に 1 つ出力する**。ログと警告は stderr。TTY でなければ進捗表示を出さない
- 形式の既定: PDF・画像は png、動画・GIF は jpeg（品質 85）。1 枚が 5MB を超えそうなら jpeg で再エンコードし、警告を出す

### `--json` manifest

```json
{"source":{"path":"/abs/report.pdf","kind":"pdf","pages":42},
 "sheets":[{"path":"/var/folders/.../gridity/report-1a2b3c/report.grid-01.png",
   "width":1288,"height":952,"est_tokens":1564,"grid":{"cols":2,"rows":2},
   "cells":[{"index":0,"label":"p.1","page":1,"rect":[12,44,432,611]}]}],
 "warnings":[]}
```

- 動画のセルには `"time": 12.5` が入る
- `rect` があるので、エージェントは「3 番目のセルの図」を特定してから再描画を指示できる
- `est_tokens` は選んだ preset の式で計算した**概算（モデルによって変わる）**

### その他

- `--dry-run`: 描画せず、シート数・グリッド・セルサイズ・概算トークンだけを出力する
- `probe`: PDF はページ数とページサイズの分布、動画は長さ・fps・表示上の解像度・回転・コーデック、GIF はフレーム数と長さ。あわせて `--detail` ごとのシート数の目安を出す

### 終了コード

| コード | 意味 |
|---|---|
| 0 | 成功 |
| 1 | 実行時エラー |
| 2 | 引数・範囲指定が不正 |
| 3 | 入力が非対応または壊れている（暗号化 PDF を含む） |
| 4 | ffmpeg などの依存が見つからない |

---

## 6. エージェント向けドキュメント（`gridity guide`）

AGENTS.md に貼り付ける前提にはしない。LLM が**自分で調べて使い方を知れる**ことを目指す。

- `gridity guide` で目次と典型的なワークフロー、`gridity guide <topic>` で詳細を出力する（Markdown を stdout に出力）
  - topic 例: `pdf`、`video`、`images`、`layout`、`output`（manifest のスキーマと終了コード）、`recipes`
- 原稿は `docs/guide/*.md` に置き、`include_str!` でバイナリに埋め込む。**CLI のバージョンとドキュメントが常に一致する**
- CLI のヘルプやエラーと揃えるため、ガイドは英語で書く
- 入口を複数用意して、LLM が guide を見つけられるようにする
  - `--help` の末尾に「For agents: run `gridity guide` for usage, recipes, and output schema」
  - エラーメッセージに該当する topic を示す（例: `see: gridity guide video`）
  - `probe` の出力に推奨コマンド例を含める
- `recipes` の内容例: 「まず `probe` → `--dry-run` で枚数とトークンを確認 → `render`」「気になるセルを `--pages N --detail high` で拡大」「長い動画は `--count` で全体を把握 → `--start/--end` で絞る」
- 同じ原稿をリポジトリの docs としても読めるので、人間向けのドキュメントも兼ねる

---

## 7. モジュール構成と crates

```
src/
  main.rs       エントリ、終了コードへの変換
  cli.rs        clap derive の定義
  detect.rs     入力種別の判定
  select.rs     --pages・時刻・--crop のパース、サンプリング（純粋関数）
  pagetext.rs   PDF ページ解析の集計と判定、テキストの組み立て（純粋関数）
  extract.rs    `text` コマンド
  layout.rs     レイアウト計算、fits()（純粋関数）
  compose.rs    シートの合成（セル・ラベル・ヘッダー）
  text.rs       フォントの読み込みと文字描画
  output.rs     エンコード、出力先とファイル名、manifest
  guide.rs      include_str! した docs/guide の出力
  source/{mod,pdf,video,anim,images}.rs
docs/guide/*.md
```

処理の流れ: probe → `Vec<Item>`（縦横比とラベル変数） → layout → シートごとにセルを描画 → 合成 → 書き出し → パスを出力

- crates: `clap`（derive）、`anyhow`、`image`、`imageproc`、`ab_glyph`、`hayro`（固定）、`rayon`、`serde`、`serde_json`
- テスト用: `assert_cmd`、`predicates`、`tempfile`
- バージョンは実装時に最新の互換セットを選ぶ

---

## 8. テスト

- **ユニットテスト**: `layout`（制約を絶対に超えない、均等に分割される）、`select`（範囲・時刻のパース、範囲外の検出）、テンプレートの展開、出力先ハッシュの決定性
- **結合テスト**
  - 動画フィクスチャは、テストの中で `ffmpeg -f lavfi -i testsrc=duration=10:size=640x360` で生成する（ffmpeg がなければスキップ）
  - 縦向き動画（rotate=90）や SAR が 1 でない動画も同じ方法で作る
  - 小さな PDF（縦横混在）と EXIF 付き JPEG のフィクスチャをリポジトリに置く
  - 検証するのは出力画像の寸法、manifest の内容、終了コード（ピクセル比較はしない）

---

## 9. マイルストーン

1. **M0 骨組み**: CLI 定義、エラーと終了コード、出力先（tmp + ハッシュ）とファイル名
2. **M1 レイアウトと合成（入力は画像ファイル群）**: 一番単純な入力で処理の流れを最後まで通す
3. **M2 PDF（hayro）**: 縦横混在、回転、暗号化 PDF のエラー
4. **M3 動画と GIF/APNG**: ffprobe / ffmpeg の並列化、表示上の寸法の扱い
5. **M4 エージェント向け機能**: `probe`、`--json`、`--dry-run`、`gridity guide` と `docs/guide`
6. **M5 仕上げ**: 速度計測と PDF の並列化の検討、5MB 上限の扱い、エッジケース

---

## 10. リスクと保留事項

- **hayro**
  - 0.x のため API が変わる可能性がある（バージョン固定で対処）
  - 速度はまだ最適化されていない
  - 埋め込まれていない CID フォントなどは、**エラーを出さずに表示が崩れる**ことがある
  - 問題が出たら pdfium を後から足せるように、境界は `render_page` だけにしておく
- **PDF 本文の読みやすさ**: compat では制約が厳しめ（§2 の試算表）
- **長い動画で細かい間隔を指定した場合**: 毎回シークする方式は遅くなる。後で 1 パスの `fps=` フィルタに切り替える余地を残す
- **tmp の掃除**: macOS は TMPDIR を定期的に掃除するが、明示的に消したい場合のために `gridity clean` を later とする
- **preset の環境変数（保留）**: `--preset` 引数だけで機能は足りる。環境変数の利点は「エージェントが毎回 `--preset hires` を付け忘れても、環境ごとに既定値を変えられる」ことだけ。必要になったら追加する
