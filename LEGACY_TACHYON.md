# 旧Tachyonの統合記録

## 対象

Codexを基に開発していたコーディングハーネスの最新作業を、一つの参照先にまとめるための記録です。新Tachyonの設計・実装は扱いません。過去の設計文書にある「次の抽出」は当時の計画であり、この保存作業の追加要件ではありません。

最終保存先は、人が選択した既存の `shute2004/tachyon` リポジトリの `main` です。これは旧Tachyonのソーススナップショットを保存するための統合であり、検証済みリリースを示すものではありません。`legacy-tachyon-2026-10-27` を安定した参照用タグとして作成する予定ですが、この記録の作成時点ではまだ存在しません。

統合入力は `7d4b2e4bc30c00be9c2e19fc68e3f6fbb1e592f3`（PR #126）、その祖先のCI基盤は `c83d6970fd7a41ae79e3503d6f6f449f8666b333`（PR #124）です。既存のGit履歴・ライセンスを保持し、ディレクトリやcrate名の一括変更は行いません。

## 変更の照合

| 元の作業 | 統合入力での扱い |
| --- | --- |
| [PR #122](https://github.com/shute2004/tachyon/pull/122) 履歴ストレージ | 後続の `HistoryTimeline<ResponsesHistoryRecord>` に置き換え済み。古い実装を重ねて導入しない。 |
| [PR #124](https://github.com/shute2004/tachyon/pull/124) CI基盤 | `c83d6970fd` が統合入力の祖先。CIの構成と履歴を含む。 |
| [PR #125](https://github.com/shute2004/tachyon/pull/125) TLS依存修正 | 後続で取り込み、`rustls 0.23.46` に更新済み。古いlockfileへ戻さない。 |
| [PR #126](https://github.com/shute2004/tachyon/pull/126) ハーネス統合 | 統合入力の本体。入力・履歴・モデル境界・MCP・Windows旧方式停止などを含む。 |
| [PR #127](https://github.com/shute2004/tachyon/pull/127) 実行出力の共有 | 元の5ファイルの変更を統合候補に収録。出力を共有し、base64のwire形式と保持上限は維持する。 |
| [PR #128](https://github.com/shute2004/tachyon/pull/128) Guardian資格情報 | 同等の変更を取り込み済み。子プロセス環境とHTTPヘッダーの双方で制限する。 |
| [PR #129](https://github.com/shute2004/tachyon/pull/129) MCPページング | Legacy/Modern両方のカーソル継続と繰り返し拒否を後続で取り込み済み。 |
| [PR #130](https://github.com/shute2004/tachyon/pull/130) Linuxのbind-data FD | 同等の変更を取り込み済み。拒否ファイルごとに別のFDを保持する。 |

この表はソースの照合結果です。元のPRが閉じられたことや、全テストが成功したことを意味しません。

追加のローカル作業は、以下のように分けています。

- `codex/fix-cell-termination-output` の `93e0bd92` と `e17a9942` は、終了処理が進捗通知を追い越す場合の既存の出力欠落修正として、元の3ファイルの変更を統合候補に収録します。新しい機能や履歴APIは追加しません。
- `codex/phase2-neutral-history-snapshot` の追加コミット `7b647fa4` は、中立な履歴参照を拡張APIへ公開する未採用の設計実験です。この整理では取り込まず、元のGit参照を保持します。同ブランチにある旧PR #122の実装を、後続実装へ重ねて取り込むこともしません。
- `ByteChunk` のbase64表現は不変ですが、PR #127のRustフィールドは `Vec<u8>` から共有された `Arc<Vec<u8>>` に変わります。この内部crateを直接利用する外部Rustコードまで、ソース互換とは主張しません。

## 検証の基準点

統合入力7d4の [blocking-ci run 38712839615](https://github.com/shute2004/tachyon/actions/runs/38712839615) は失敗で終了しています。

- Windows GNU向けリリースビルド、cargo-deny、リポジトリ検査、整形などのジョブは成功しました。
- SDKビルドはTUIの無条件コードから参照する型のimport不足で失敗しました。
- LinuxテストはgRPCテストの `assert_eq!` マクロの曖昧さでビルドに失敗しました。
- Windows Clippyは3箇所の `.err().expect()` で失敗しました。
- 他にもWindowsの実行時失敗とタイムアウト、Linux/macOSのキャンセル、native MSVCのスキップがあります。

この整理では、上の3種類の明確なコンパイル・lint不整合を最小限で修正しました。失敗した実行時テストを削除したり、タイムアウトを緩めたりして成功扱いにはしていません。修正済み候補に対するRustのコンパイル、テスト、Clippyは実行していません。したがって、上記runは修正前の基準点であり、この候補が検証済みリリースであることは示しません。

## 残る制限

- Codex/OpenAI互換の投影・アダプターやUI・製品依存は残っています。独立した汎用カーネルへの抽出完了は主張しません。
- ネイティブなJSONツール結果やバイト列メディアを、履歴保存から再開・要求生成まで一貫して保持する移行は未完了です。
- Windowsの同一ユーザー旧サンドボックスは、削除の境界を保証できないため停止しています。既存の専用アカウント方式には、明示的な管理者権限でのセットアップが必要です。通常実行への自動フォールバックや設定の自動移行はありません。
- Windowsのforeground Ctrl-Cとshell-commandタイムアウトは、統合入力の時点で未解決です。
- スキップ・キャンセル・未実施のテストを、検証済みとは扱いません。

## 作業データの保全

整理前に、主チェックアウトと11個の機能別作業ツリーに残っていた83パスを、ローカル専用の `codex/legacy-preservation/20261025-*` 保存点へ取り込みました。元の作業ファイル・index・ブランチを変更せずに保存しています。

これらにはローカルの設定・作業文脈も含むため、保存点を一括でリモートへpushしてはいけません。元の作業ツリーを整理する場合も、未統合の実験とローカル設定を復元できることを先に確認します。ビルドキャッシュはソースと区別し、ソース・履歴の代わりに保存することはしません。

## ビルドと検証

旧Tachyon自身をビルドする場合は、このリポジトリから始めます（上流Codexのインストーラーとは別です）。

以下では、統合コードの保存点 `24215daf26f9cf8ba2924e8983d08733a6ead7a4` を明示してcheckoutします。後からmainやブランチが更新されても、同じソースを取得できます。

```sh
git clone https://github.com/shute2004/tachyon.git legacy-tachyon
git -C legacy-tachyon checkout --detach 24215daf26f9cf8ba2924e8983d08733a6ead7a4
cd legacy-tachyon/codex-rs
CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0 \
  CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 \
  rustup run 1.95.0 cargo build --locked -p codex-cli --bin codex
```

Rustのツールチェーンは `rust-toolchain.toml` の1.95.0を使用します。別途インストールされたCargoがPATHで優先される環境でも固定版を使うため、上の例では `rustup run` を明示しています。必要なツールやプラットフォーム依存は、当時の [`docs/install.md`](docs/install.md) も参照できますが、そこにあるclone先は上流Codexであり、このリポジトリに読み替える必要があります。

テストは `codex-rs` で `just test -p <対象crate>` を使用します。`just fmt` / `just fmt-check` は整形確認であって、コンパイルや実行の証明ではありません。

重いRust検証の開始前には、共有・外部のtarget/cacheも含めてサイズを計測し、最低30 GiBの空きを確保します。生成物は8 GiBで警告、10 GiBで停止します。これはCargoの強制クォータではなく運用上限です。ビルドが収まらない場合は別ボリュームや承認済みCIを使い、チェックアウト・履歴・未追跡ソースを削除して空きを作らないでください。
