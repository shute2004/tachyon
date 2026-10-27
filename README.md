# Legacy Tachyon（旧Tachyon）

このリポジトリは、Codexを基に開発していた旧Tachyonのコーディングハーネスを保存・整理するためのものです。別方針で始める新Tachyonとは区別します。

既存の実装と履歴をまとめることが目的です。未完成だったカーネル抽出の続行、機能追加、上流Codexへの全面追従は、この整理には含めません。

## 内容と状態

- Rustのハーネス・実行基盤: [`codex-rs/`](codex-rs/)
- SDK: [`sdk/`](sdk/)
- 統合元・検証結果・既知の制限: [`LEGACY_TACHYON.md`](LEGACY_TACHYON.md)
- 当時の設計と抽出方針: [`TACHYON.md`](TACHYON.md)、[`docs/tachyon/architecture.md`](docs/tachyon/architecture.md)

モデル・UIからの独立は途中段階です。Codex互換の実装や命名を残しており、完成済みの独立カーネルや全プラットフォーム検証済みのリリースとは扱いません。検証の成功・失敗・未実施は、統合記録で区別します。

## ソースからの利用

このチェックアウトの [`codex-rs/rust-toolchain.toml`](codex-rs/rust-toolchain.toml)、[`codex-rs/Cargo.lock`](codex-rs/Cargo.lock)、Bazelの固定ファイルを使用してください。ビルド・テスト手順とディスク上限は[統合記録](LEGACY_TACHYON.md#ビルドと検証)に記載しています。

元のCodexの紹介・インストール案内は [`README.codex.md`](README.codex.md) にそのまま保存しています。そこにあるOpenAIのインストーラーや配布パッケージは、旧Tachyonのビルドをインストールするものではありません。

## ライセンス

Codex由来の著作権表示とライセンスは保持しています。[Apache-2.0](LICENSE) と [NOTICE](NOTICE) を参照してください。
