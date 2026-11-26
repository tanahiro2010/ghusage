# CLI リファレンス {#cli-reference}

```text
ghusage [OPTIONS] [COMMAND]
```

コマンドを指定しないと、`ghusage` は `overview` を実行します。グローバルオプションは、コマンドの前にも後ろにも置けます(`ghusage --since 30d daily` と `ghusage daily --since 30d` は同じです)。

## コマンド {#commands}

| コマンド | 説明 | 既定の期間 |
|---|---|---|
| `overview` | 期間の概要(既定のコマンド) | 直近 7 日 |
| `daily` | 日ごとの活動 | 直近 7 日 |
| `weekly` | 週ごとの活動(週は月曜日から始まります) | 直近 8 週 |
| `monthly` | 暦月ごとの活動 | 直近 6 か月 |
| `yearly` | 暦年ごとの活動 | 直近 3 年 |
| `repos` | リポジトリごとの活動 | 直近 7 日 |
| `prs` | 期間内に活動のあったプルリクエスト | 直近 7 日 |
| `reviews` | 自分が提出したレビューと、自分が受けたレビュー | 直近 7 日 |
| `comments` | 自分が書いたコメントと、自分のプルリクエストに付いたコメント | 直近 7 日 |
| `issues` | 自分が作成・クローズ・再オープンした Issue | 直近 7 日 |
| `pr <URL>` | 1つのプルリクエストの詳細とタイムライン | (期間に依存しません) |
| `commit <COMMIT>` | 1 つのコミット。メッセージ、作者とコミッター、親コミット、変更ファイル。`--patch` を付けると diff も出す | (期間に依存しません) |
| `tui` | 対話型のターミナル UI | 直近 30 日 |
| `sync` | GitHub からデータを取得してローカルのキャッシュに入れる | 直近 30 日 |
| `doctor` | 認証、キャッシュ、接続を確認する | |
| `auth status` | どの認証情報を使っているか、それが有効かを表示する | |
| `cache status` | キャッシュの場所、サイズ、最後の同期、カバレッジを表示する | |
| `cache clear` | キャッシュした GitHub のデータをすべて削除する(認証情報は消えません) | |

「直近 7 日」は、今日とその前の 6 日間です。ローカル時刻の午前0時から始まり、現在時刻で終わります。「直近 8 週」は今週の 7 週前の月曜日から、「直近 6 か月」は今月の 5 か月前の月の1日から始まります。詳しくは[期間とタイムゾーン](./time.md)を参照してください。

### レポートのコマンド {#report-commands}

`overview`、`daily`、`weekly`、`monthly`、`yearly`、`repos`、`prs`、`reviews`、`comments`、`issues` は、期間のうちまだ取得していない範囲を同期してから(`--offline` のときを除く)、レポートとそのカバレッジを表示します。それぞれの数値の意味は[指標の定義](./metrics.md)を参照してください。

- `overview`: 期間の概要と、上位 5 つのリポジトリ。
- `daily` / `weekly` / `monthly` / `yearly`: 暦の区切りごとに1行と、合計の行。週は ISO 週番号(`2026-W47`)、月は `2026-11` の形で表示します。
- `repos`: 期間内に活動のあったリポジトリごとに1行。自分の活動が多い順に並びます。公開範囲の後ろの `*` は自分が所有するリポジトリ、`(no access)` は同期中に GitHub がアクセスを拒否した(HTTP 403 または 404)リポジトリを示します。拒否されたリポジトリでも、キャッシュ済みのデータは残します。
- `reviews`: 状態ごとの件数に続けて、自分が提出したレビューと、自分のプルリクエストにほかの人が提出したレビュー。
- `comments`: 種類ごとの件数に続けて、自分が書いたコメントと、自分のプルリクエストで受けたコメントの一つひとつ。
- `issues`: 件数、自分が作成した Issue のうちオープンなものの数(現在の状態)、期間内に自分が作成・クローズ・再オープンした Issue の一つひとつ。

### `prs`

```text
ghusage prs [--details] [--sort <SORT>] [--state <STATE>] [--all-participants]
```

期間内に活動のあったプルリクエストを一覧にします。

| オプション | 説明 |
|---|---|
| `--details` | プルリクエストごとの活動の統計と、自分のプルリクエストのライフサイクルの所要時間も表示します |
| `--sort <SORT>` | `activity`(既定。期間内のイベント数)、`comments`、`reviews`、`created`(作成日時)、`updated`(期間内の最後の活動)。常に大きい順・新しい順です |
| `--state <STATE>` | 現在の状態が `open`、`closed`(マージせずにクローズ)、`merged` のプルリクエストだけを表示します |
| `--all-participants` | 期間内にほかの人だけが活動を残したプルリクエストも含めます |

既定では、期間内に自分のイベントが1つ以上あるプルリクエストだけを表示します。列の意味は[指標の定義](./metrics.md#pull-request-list)にあります。

`Changes` 列には、各プルリクエストの base と head の差分を `+A -D` の形で表示します(分からないときは `?`)。これは最後に同期した時点のスナップショットで、期間中に行った変更ではありません。表の下にもその旨の注記を出します。[プルリクエストの変更行数](./metrics.md#pull-request-changes)を参照してください。

### `pr`

```text
ghusage pr <URL> [--timeline] [--event-type <TYPES>]...
```

1つのプルリクエストについて、状態、作成者、マージ、コミット(プルリクエストにもう含まれていないコミットも含む)、状態ごとのレビュー、種類ごとのコメント、レビュースレッド(現在の状態)、最新の CI の結果、参加者、所要時間、カバレッジを表示します。`Changes (current snapshot)` の行には、プルリクエストの差分を `+A -D in F files (base to head as of <時刻>)` の形で表示します。まだ分からないときは、その旨を表示します。

| オプション | 説明 |
|---|---|
| `<URL>` | `https://github.com/OWNER/REPO/pull/NUMBER`。後ろに `/files` などのパス、クエリ文字列、フラグメントが付いていても受け付けます |
| `--timeline` | すべてのイベントを順に表示します(表形式の出力)。JSON と CSV には常にタイムラインが入ります |
| `--event-type <TYPES>` | タイムラインに出すイベントの種類を絞ります。カンマ区切りでも、オプションの繰り返しでも指定できます。件数には影響しません |

オンラインのときは、表示する前に必ずプルリクエストを取得し直します。取得に失敗し、そのプルリクエストがキャッシュにあれば、警告を出してキャッシュのデータを使います。`--offline` のときは、プルリクエストがすでにキャッシュにある必要があります。自分が関わっていないプルリクエストを表示しても、自分のレポートには加わりません。

イベントの種類: `commit_authored`、`pr_opened`、`pr_converted_to_draft`、`pr_ready_for_review`、`pr_closed`、`pr_reopened`、`pr_merged`、`pr_commit`、`pr_head_force_pushed`、`review_submitted`、`review_dismissed`、`conversation_comment`、`inline_comment`、`review_summary`、`review_requested`、`review_request_removed`、`assigned`、`unassigned`、`labeled`、`unlabeled`、`milestoned`、`demilestoned`、`renamed`、`issue_opened`、`issue_closed`、`issue_reopened`、`check_run`、`commit_status`。(`commit_authored` には、以前の名前の `commit` も引き続き使えます。)

```sh
ghusage pr https://github.com/OWNER/REPO/pull/123 --timeline \
  --event-type review_submitted,inline_comment
```

### `commit` {#commit}

```text
ghusage commit <COMMIT> [--patch]
```

1 つのコミットを表示します。表示するのは、リポジトリ、SHA、作者とコミッター(git の名前、ひも付いた GitHub のログイン名、日時)、親コミット(2 つ以上あればマージコミット)、メッセージの全文、変更ファイルの一覧です。変更ファイルには、状態(`added`、`modified`、`removed`、`renamed` など)、追加・削除した行数、合計を付けます。

| オプション | 説明 |
|---|---|
| `<COMMIT>` | `OWNER/REPO@SHA` か `https://github.com/OWNER/REPO/commit/SHA`(`.../pull/NUMBER/commits/SHA` も可)。SHA は 7 桁以上の 16 進数に省略できる |
| `--patch` | 各ファイルの diff も出す。表形式では、色が有効なら追加した行を緑、削除した行を赤、hunk ヘッダーをシアンで表示する。JSON では各ファイルに `patch` フィールドが付く |

コミットは、前に見たことがあれば(このコマンドでもターミナル UI でも)キャッシュから読みます。なければ GitHub から `GET /repos/{owner}/{repo}/commits/{sha}` で取得し、ファイルが複数ページにわたるときは全ページをたどって、キャッシュに保存します。コミットの中身がレポートの数値を変えることはありません。`--offline` のときは、コミットがすでにキャッシュにある必要があります。

GitHub は、バイナリファイルや大きすぎる diff を返しません。そのようなファイルは「no diff available from GitHub」と表示します。また、GitHub が 1 つのコミットについて返すファイルは 3000 件までです。それを超えて欠けたファイルがあるときはその旨を表示します(JSON では `files_truncated`)。`--public-only` のときは、公開されていないリポジトリのコミットを拒否します。

```sh
ghusage commit OWNER/REPO@1a2b3c4 --patch
ghusage commit https://github.com/OWNER/REPO/commit/1a2b3c4d5e6f --format json
```

### `sync`

```text
ghusage sync [--full] [--diff-stats]
```

期間のデータを取得してキャッシュに入れます。`--full` を付けないと、まだ取得し終えていない範囲だけを取得します。`--full` を付けると、期間全体を取得し直します。`--offline` とは併用できません。[同期とキャッシュ](../guide/syncing.md)も参照してください。

| オプション | 説明 |
|---|---|
| `--full` | まだ取得していない範囲だけでなく、期間全体を取得し直します |
| `--diff-stats` | 期間内の自分のコミットのうち、まだ行数が分からないものについて、追加・削除した行数も取得します(100 コミットごとに GraphQL のリクエスト 1 回)。アカウントの最初の同期と CI(`CI` が設定されているとき)では、付けなくても取得します(`--no-diff-stats` を参照)。[行数の取得](../guide/syncing.md#line-counts)を参照してください |

行数を取得したときは、出力に `line counts: N commits (already known K, fetched F, unavailable U)` の行が加わり、JSON の出力には `diff_stats` オブジェクトが入ります([JSON の出力](./json.md#sync)を参照)。

フィルター(`--repo`、`--owner`、`--visibility`、`--ownership`)を指定しても、`sync` が取得する範囲は絞られません。

### `tui`

対話型のターミナル UI を開きます。[ターミナル UI](../guide/tui.md) を参照してください。

### `doctor`

キャッシュを開けるか、整合性チェックに通るかを確かめ、最後の同期を表示します。`--offline` でなければ、認証情報が見つかるか、GitHub が受け付けるか、レート制限が残っているかも確かめます。いずれかのチェックに失敗すると、終了コード 1 で終わります。

```text
$ ghusage doctor --offline
ok   cache opened: /Users/me/Library/Application Support/dev.ghusage.ghusage/ghusage.db
ok   cache integrity: ok
ok   offline: GitHub checks skipped
```

### `auth status`

ログイン名、認証情報の取得元、トークンのスコープ(GitHub が返した場合)、レート制限を表示します。認証情報が見つからないとき、または GitHub が拒否したときは、終了コード 3 で失敗します。

### `cache status` / `cache clear`

`cache status` は、データベースのパスとサイズ、スキーマのバージョン、リポジトリ・プルリクエスト・Issue・イベントの件数、最後の同期、直近 30 日のカバレッジを表示します。`cache clear` は、キャッシュしたデータをすべて削除します。

```text
$ ghusage cache status
Cache: /Users/me/Library/Application Support/dev.ghusage.ghusage/ghusage.db
  size: 0.2 MiB, schema version 1
  repositories 0, pull requests 0, issues 0, events 0
  last sync: never
```

## グローバルオプション {#global-options}

| オプション | 説明 |
|---|---|
| `--since <SINCE>` | 期間の開始。`YYYY-MM-DD`、RFC 3339、`today`、`yesterday`、または `Nd` / `Nw` / `Nm` / `Ny`(直近 N 日・週・月・年) |
| `--until <UNTIL>` | 期間の終了(この時刻は含みません)。日付を指定すると、その日がまるごと含まれます。既定は現在時刻です。ここには `Nd`/`Nw`/`Nm`/`Ny` を指定できません |
| `--timezone <TZ>` | 日・週・月の境界に使う IANA タイムゾーン(既定: `TZ`、次にシステムの設定、最後に UTC) |
| `--repo <OWNER/NAME>` | このリポジトリだけ(大文字と小文字を区別しません)。繰り返し指定できます |
| `--owner <LOGIN>` | このユーザーまたは organization が所有するリポジトリだけ。繰り返し指定できます |
| `--visibility <public\|private>` | 公開リポジトリだけ、または公開でない(private と internal の)リポジトリだけ |
| `--ownership <owned\|external>` | 自分が所有するリポジトリだけ、またはそれ以外のリポジトリだけ |
| `--user <USER>` | ほかのユーザーの活動を分析します(自分のトークンで見える範囲に限られます) |
| `--format <table\|json\|csv\|markdown>` | 出力形式(既定は `table`)。`csv` を使えるのはレポートと `pr` だけです。`sync`、`doctor`、`auth`、`cache`、`tui` に指定すると、終了コード 2 でエラーになります。`markdown` を使えるのは `overview`、`daily`、`weekly`、`monthly`、`yearly`、`repos` だけです([プロフィール README](../guide/profile-readme.md)を参照)。JSON や CSV と同じく、`--include-private` を付けない限りプライベートリポジトリの名前を隠します |
| `--offline` | キャッシュのデータだけを使い、GitHub には接続しません |
| `--no-cache` | ローカルのキャッシュではなく、一時的なキャッシュに最新のデータを取得します |
| `--require-complete` | 期間内に不完全なデータが1つでもあれば、終了コード 6 で終わります |
| `--no-color` | 色を付けません(`NO_COLOR` も参照します) |
| `--include-private` | JSON / CSV の出力に、private リポジトリの名前とタイトルを含めます |
| `--public-only` | 公開リポジトリだけを取得・表示します |
| `--commit-date <committer\|author>` | コミットがどの日に入るか、期間内かどうかを、どちらのコミット日付で決めるか(既定は `committer`)。ターミナル UI でも使います |
| `--token-file <PATH>` | GitHub のトークンをこのファイルから読みます(ほかのユーザーが読めない権限にしておく必要があります) |
| `--data-dir <PATH>` | キャッシュのデータベースを置くディレクトリ(既定: OS のユーザーデータディレクトリ、または `GHUSAGE_DATA_DIR`) |
| `--no-diff-stats` | アカウントの最初の同期と CI で、行数を既定で取得しないようにします(レポートが自動で行う同期も含む)。`sync` に `--diff-stats` を付けた場合は取得します |
| `-j`、`--jobs <N>` | GitHub への同時リクエスト数の上限。1〜16(既定は 8、または `GHUSAGE_JOBS`)。大きくすると短い期間の初回同期が速くなりますが、メモリを多く使い、GitHub の二次レート制限に当たりやすくなります。以前の名前の `--concurrency` も使えます |
| `--max-age <AGE>` | キャッシュの同期からこの時間が経っていなければ、レポートは同期をせず、その同期の時点のデータを表示します。`30s`、`5m`、`1h`、または常に最新を取得する `0`(既定は `5m`、または `GHUSAGE_MAX_AGE`) |
| `-h`, `--help` | ヘルプを表示します |
| `-V`, `--version` | バージョンを表示します |

フィルターは AND で組み合わさり、リポジトリは指定したフィルターすべてに一致する必要があります。`--repo` や `--owner` を繰り返したときは、その値どうしは OR で組み合わさります。「所有する」とは、リポジトリのオーナーが自分であるか、(自分を分析しているときに)自分がそのリポジトリの admin 権限を持っていることです。リポジトリを所有する organization のメンバーであるだけでは、所有するリポジトリにはなりません。

グローバルオプションはどのコマンドでも受け付けますが、すべてのコマンドが使うわけではありません。期間(`--since`、`--until`)を使うのは、レポート、`sync`、`tui` です。フィルター(`--repo`、`--owner`、`--visibility`、`--ownership`)と `--commit-date` を使うのは、レポートと `tui` だけです。コマンドが使わないオプションを指定すると、`ghusage` は `` warning: --repo has no effect on `sync` `` のような警告を stderr に出し、そのまま処理を続けます。

## 出力とターミナル {#output-and-terminals}

- レポートは stdout に出力します。エラー、警告、補足のメッセージ、同期の進み具合は stderr に出力します。
- 色を付けるのは、stdout がターミナルで、`--no-color` が指定されておらず、`NO_COLOR` が空でない値に設定されていないときだけです。パイプに渡した出力には、エスケープシーケンスは入りません。
- 同期の進み具合は、stderr がターミナルのときだけ表示します。
- GitHub から取得したテキスト(タイトル、ログイン名、名前)は、制御文字を取り除いてから表示します。
- パイプが閉じられても(たとえば `ghusage daily | head -3`)、エラーとしては扱いません。
- 表形式の出力は人が読むためのもので、バージョンによって変わることがあります。スクリプトでは `--format json` か `--format csv` を使ってください。[JSON と CSV の出力](./json.md)を参照してください。

## 終了コード {#exit-codes}

| コード | 意味 |
|---|---|
| `0` | 成功 |
| `1` | 一般的なエラー(例: オフラインモードでキャッシュにアカウントがない、プルリクエストがキャッシュにない、`doctor` のチェックに失敗した、ターミナルのエラー) |
| `2` | 引数が無効(不明なオプションや値、無効な日付やタイムゾーン、空の期間、無効なプルリクエストの URL、不明なイベントの種類、`sync --offline`、CSV 出力のないコマンドへの `--format csv`、Markdown 出力のないコマンドへの `--format markdown`) |
| `3` | 認証エラー(認証情報がない、トークンファイルが使えない、GitHub がトークンを拒否した) |
| `4` | GitHub API のエラー(ネットワーク、レート制限、サーバーエラー)。`sync` で一部のデータを最後まで取得できなかったときも、このコードになります。レポートのコマンドは、GitHub に接続できなくてもアカウントがキャッシュにあれば失敗せず、警告を出してキャッシュのデータを表示します |
| `5` | キャッシュのエラー(データベースを作成できない・開けない、データベースが壊れている・新しすぎる、別の同期が実行中) |
| `6` | `--require-complete` を指定していて、データが不完全(レポートは出力されます) |

`--help` と `--version` は終了コード 0 で終わります。

現在のバージョンでは、次のように表示されます。

```text
$ ghusage --since 0d
error: invalid date or time: 0d (use YYYY-MM-DD, RFC 3339, today, yesterday, or Nd/Nw/Nm/Ny)
$ echo $?
2

$ ghusage --timezone Mars/Phobos
error: unknown time zone: Mars/Phobos

$ ghusage sync --offline
error: sync cannot run with --offline

$ ghusage --offline --no-cache
error: the argument '--offline' cannot be used with '--no-cache'
```

## 環境変数 {#environment-variables}

| 変数 | 説明 |
|---|---|
| `GH_TOKEN` | GitHub のトークン(優先順位は `--token-file` の次) |
| `GITHUB_TOKEN` | GitHub のトークン(`GH_TOKEN` が設定されていないときに使います) |
| `GHUSAGE_DATA_DIR` | キャッシュを置くデータディレクトリ(`--data-dir` が優先します) |
| `GHUSAGE_MAX_AGE` | `--max-age` の既定値 |
| `GHUSAGE_JOBS` | `--jobs` の既定値 |
| `CI` | 設定されていると(空、`false`、`0` 以外)、アカウントの最初の同期と同じように、すべての同期で行数も取得します。`--no-diff-stats` で止められます |
| `GHUSAGE_API_URL` | GitHub Enterprise Server 向けの REST API のベース URL(既定は `https://api.github.com`)。GraphQL のエンドポイントもこれに合わせて変わり、URL が `/api/v3` で終わるときは `https://HOST/api/graphql`、それ以外は `<GHUSAGE_API_URL>/graphql` になります |
| `GHUSAGE_GRAPHQL_URL` | GraphQL のエンドポイントの URL(`GHUSAGE_API_URL` から決まる URL より優先します) |
| `GHUSAGE_LOG` | stderr に出す診断用ログのフィルター。`tracing` のフィルター構文で、`debug` や `ghusage_github=debug` のように書きます(既定は `warn`)。debug では GitHub へのリクエストを、GraphQL の操作名と、403/429 の応答ではレート制限のヘッダーとエラーメッセージと合わせて出力します。トークンは出力しません |
| `TZ` | `--timezone` を指定しないときの既定のタイムゾーン(`Europe/Berlin` のような IANA 名) |
| `NO_COLOR` | 空でない値を設定すると、色を付けません |

GitHub Enterprise Server については、[認証](../guide/authentication.md#github-enterprise-server)を参照してください。
