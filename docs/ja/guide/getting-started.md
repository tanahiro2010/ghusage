# はじめに {#getting-started}

`ghusage` は、開発者の GitHub での活動を分析するツールです。対象はコミット、プルリクエスト、レビュー、コメント、Issue と、プルリクエストの中で起きた活動です。GitHub API から取得したデータをローカルの SQLite キャッシュに保存し、そのキャッシュからレポートを作ります。

## インストール {#install}

### npm {#npm}

```sh
npm install -g ghusage
ghusage --version
```

npm パッケージの `ghusage` は、小さな Node.js 製のランチャーです。お使いのプラットフォーム向けにビルド済みのネイティブバイナリが optional dependency としてインストールされ、ランチャーが引数を渡してそのバイナリを実行します。Node.js 18 以降が必要です。

| プラットフォーム | npm パッケージ |
|---|---|
| macOS(Apple Silicon) | `@ghusage/ghusage-darwin-arm64` |
| macOS(Intel) | `@ghusage/ghusage-darwin-x64` |
| Linux x64(静的リンク) | `@ghusage/ghusage-linux-x64` |
| Linux arm64(静的リンク) | `@ghusage/ghusage-linux-arm64` |
| Windows x64 | `@ghusage/ghusage-win32-x64` |
| Windows arm64 | `@ghusage/ghusage-win32-arm64` |

インストールするのは `ghusage` だけで構いません。上の表のうち環境に合うパッケージを npm が自動で選びます。

最新のリリースに更新するには、次を実行します。

```sh
npm install -g ghusage@latest
```

npm がプラットフォーム用のパッケージをインストールしなかった場合(起きる理由は[ネイティブパッケージが見つからない場合](#missing-native-package)を参照)は、初めて `ghusage` を実行したときにランチャーがそのパッケージを取得します。

- ランチャーは `npm pack @ghusage/ghusage-<platform>@<version>` を実行します。そのため、利用しているレジストリやレジストリのプロキシ、認証、npm の整合性チェックといった npm の設定がいつもどおり適用されます。取得中は stderr に 1 行だけ表示します。たとえば
  `ghusage: fetching @ghusage/ghusage-darwin-arm64@0.1.1 ...` です。
- 取得したパッケージのうちバイナリだけをユーザーごとのキャッシュに残し、以後はそれを使います。

  | OS | キャッシュディレクトリ |
  |---|---|
  | macOS | `~/Library/Caches/ghusage` |
  | Linux | `$XDG_CACHE_HOME/ghusage` または `~/.cache/ghusage` |
  | Windows | `%LOCALAPPDATA%\ghusage\cache` |

  バイナリはその下の `native/<version>/<platform>/` に置かれます。別のディレクトリを使うには `GHUSAGE_CACHE_DIR` を設定します。このディレクトリは削除しても問題ありません。必要になったときにまた作られます。
- この自動取得を止めるには `GHUSAGE_NO_AUTO_INSTALL=1` を設定します。その場合 `ghusage` は理由を表示して終了します。

ほかに次の点に注意してください。

- 上の表にないプラットフォームでは、ランチャーはメッセージを表示して終了します。ソースからビルドしてください。
- `GHUSAGE_BINARY` に `ghusage` バイナリのパスを設定すると、ランチャーはそのバイナリを実行します(自分でビルドしたものなど)。インストール済みのバイナリやキャッシュにあるバイナリよりも優先されます。

### ネイティブパッケージが見つからない場合 {#missing-native-package}

`ghusage` が `the native binary for <platform> is missing` と表示して終了した場合、npm はランチャーをインストールしたものの、同じバージョンのプラットフォーム用パッケージをインストールしておらず、自動取得も失敗しています。よくある原因は次のとおりです。

- リリースが公開された直後で、利用しているレジストリがまだプラットフォーム用パッケージを配信していない。
- npm のレジストリが、新しいバージョンをしばらく止めておくプロキシやミラー、セキュリティツールである。この場合、新しいランチャーは配信されても、新しいプラットフォーム用パッケージはまだ配信されない。
- optional dependency が省かれた(`npm install --omit=optional` や `--no-optional`)。
- リリースからの最低経過期間の設定(`npm --before`、`--min-release-age`、mise の `minimum_release_age`)によって、プラットフォーム用パッケージが除外された。
- `GHUSAGE_NO_AUTO_INSTALL` が設定されていて、ランチャーが取得を試みなかった。

直すには、次の手順を試してください。

1. 同じバージョンを入れ直し、npm にプラットフォーム用パッケージをインストールさせます(`0.1.1` はメッセージに出たバージョンに置き換えてください)。

   ```sh
   npm uninstall -g ghusage && npm install -g ghusage@0.1.1
   ```

2. npm が使っているレジストリを確認します。

   ```sh
   npm config get registry
   ```

   `https://registry.npmjs.org/` ではなくプロキシやミラーになっている場合は、新しいプラットフォーム用パッケージが配信されるまで待つか、その間は `ghusage` の 1 つ前のバージョンをインストールしてください(バージョンの一覧は `npm view ghusage versions` で確認できます)。

   ```sh
   npm install -g ghusage@<previous version>
   ```

3. 公開されている npm レジストリから直接インストールすることもできます。

   ```sh
   npm install -g ghusage@0.1.1 --registry https://registry.npmjs.org
   ```

   組織でレジストリのプロキシを使うよう決められている場合は、その方針に従ってください。

### ソースから {#from-source}

Rust のツールチェーン(Rust 1.89 以降)が必要です。

リポジトリから直接インストールするには、次を実行します。

```sh
cargo install --git https://github.com/tanahiro2010/ghusage
```

ローカルにチェックアウトしてビルドすることもできます。

```sh
git clone https://github.com/tanahiro2010/ghusage
cd ghusage
cargo build --release
./target/release/ghusage --version
```

::: info 対応プラットフォーム
`ghusage` は macOS(Apple Silicon)で開発し、使っています。コードは移植性を考えて書いてあり、CI では Linux(x86_64 と ARM64)、macOS(ARM64 と Intel)、Windows でビルドとテストを行っています。ただし、Linux と Windows の実機での動作はまだ手作業では確認していません。問題を見つけたら報告してください。
:::

## 認証 {#authenticate}

[GitHub CLI](https://cli.github.com/) を使っていてログイン済みなら、何もする必要はありません。`ghusage` は `gh auth token` を使います。そうでなければ `GH_TOKEN`(または `GITHUB_TOKEN`)を設定するか、`--token-file` を指定してください。詳しくは[認証](./authentication.md)を参照してください。

```sh
ghusage auth status
```

```text
Logged in as octocat (credentials from gh auth token)
  token scopes: gist, read:org, repo
  rate limit: core 4998/5000 (resets 2026-11-21T11:02:13+00:00), search 30/30, graphql 4990/5000
```

## 最初の同期 {#first-sync}

```sh
ghusage sync
```

`sync` は既定で直近 30 日分(今日とその前の 29 日間)を取得します。もっと長い範囲を取得するには `--since` を使います。

```sh
ghusage sync --since 2026-01-01
```

活動の多いアカウントでは、最初の同期に時間がかかることがあります。2 回目以降の同期では、足りない部分と変更された部分だけを取得します。詳しくは[同期とキャッシュ](./syncing.md)を参照してください。

先に `sync` を実行しておく必要はありません。レポートのコマンドは、足りない範囲を自動で取得します。明示的に `sync` を実行すると、進み具合と結果を確認できます。

## レポート {#reports}

```sh
ghusage                       # overview of the last 7 days (default command)
ghusage overview --since 30d
ghusage daily --since 14d
ghusage weekly                # last 8 weeks
ghusage monthly               # last 6 calendar months
ghusage yearly                # this year and the 2 before
ghusage repos
ghusage prs --details
ghusage reviews
ghusage comments
ghusage issues
ghusage pr https://github.com/OWNER/REPO/pull/123 --timeline
```

どのレポートも最後にカバレッジの行を表示します。`Coverage: complete` は、期間全体について、すべての種類のデータを取得し終えていることを表します。それ以外の場合は、取得できていない範囲や一部しか取得できていない範囲を一覧にします。

```text
Coverage: not_yet_synced  (last sync: -)
  commits: not_yet_synced — not_yet_synced 2026-11-01 00:00–2026-11-04 00:00
  pull_requests: not_yet_synced — not_yet_synced 2026-11-01 00:00–2026-11-04 00:00
  issues: not_yet_synced — not_yet_synced 2026-11-01 00:00–2026-11-04 00:00
```

## 期間の指定 {#choosing-a-period}

```sh
ghusage overview --since 2026-11-01 --until 2026-11-30   # whole of November
ghusage daily --since 7d                                 # today and the 6 days before
ghusage weekly --since 4w                                # this week and the 3 before
ghusage overview --since yesterday --until yesterday     # just yesterday
ghusage daily --timezone America/New_York
```

`--until` に日付を指定すると、その日の終わりまでが含まれます。詳しくは[期間とタイムゾーン](../reference/time.md)を参照してください。

## 絞り込み {#filtering}

```sh
ghusage repos --owner my-org
ghusage prs --repo my-org/api --repo my-org/web
ghusage overview --visibility public
ghusage overview --ownership external     # repositories you do not own
```

絞り込みはレポートにだけ適用されます。`sync` は常に、期間内のデータをすべて取得します。

## スクリプトとエクスポート {#scripts-and-exports}

```sh
ghusage weekly --format json > weekly.json
ghusage prs --format csv > prs.csv
ghusage overview --format json --require-complete || echo "data incomplete"
```

JSON と CSV では、`--include-private` を指定しない限り、プライベートリポジトリの名前とタイトルを伏せます。詳しくは[JSON と CSV の出力](../reference/json.md)を参照してください。

## オフラインでの利用 {#offline-use}

```sh
ghusage overview --offline
```

`--offline` を指定すると GitHub には一切接続しません。一度も同期していない範囲は `not_yet_synced` と表示され、stderr に注意書きが出ます。

## 次に読むもの {#next-steps}

- 各数値が何を表すのかは、[指標の定義](../reference/metrics.md)で確認できます。
- 数値を頼りにする前に、[既知の制約](../reference/limitations.md)を読んでください。
- `ghusage tui` で[ターミナル UI](./tui.md) を試せます。
