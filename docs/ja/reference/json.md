# JSON と CSV の出力 {#json-and-csv-output}

スクリプトでは `--format json` か `--format csv` を使ってください。表形式の出力は人が読むためのもので、変わることがあります。JSON のスキーマにはバージョンが付いています。

```sh
ghusage weekly --format json > weekly.json
ghusage prs --format csv > prs.csv
```

`--include-private` を指定しない限り、JSON と CSV では private リポジトリの名前、タイトル、URL を伏せます。[private リポジトリ](#private-repositories)を参照してください。

## 決まりごと {#conventions}

- **スキーマのバージョン。** どのレポートにも `"schema_version": 1` が入ります。互換性を保つフィールドの追加は、バージョンを変えずに行うことがあります。フィールドを削除したり意味を変えたりするときは、バージョンを変えます。
- **タイムスタンプ**は RFC 3339 / ISO 8601 の文字列です。`period.start` と `period.end` はレポートのタイムゾーンのオフセットを使い(`2026-11-02T00:00:00+09:00`)、それ以外のタイムスタンプはすべて UTC です(`2026-11-01T15:00:00Z`)。
- **時間の長さ**は秒単位の整数です(`_secs` で終わるフィールドと、所要時間の統計)。
- **`null` と `0` の違い。** 件数は、何も起きなかったときに `0` になります。`null` は、不明または該当しないことを表します。たとえば、最後の活動がない、マージされていない、CI の結果がない、対象のプルリクエストが0件で統計を出せない、アカウント ID が不明、といった場合です。
- **フィールドの順序**は変わりませんが、順序に依存しないでください。配列には決まった順序があり、それぞれ下で説明します。
- コマンドに固有のセクションは、そのコマンドが出力しないときは(`null` にするのではなく)**省きます**。

## レポートのオブジェクト {#report-object}

期間を対象にするレポート(`overview`、`daily`、`weekly`、`monthly`、`yearly`、`repos`、`prs`、`reviews`、`comments`、`issues`)は、次のルートフィールドを持つ JSON オブジェクトを1つ出力します。

| フィールド | 型 | 説明 |
|---|---|---|
| `schema_version` | integer | `1` |
| `account` | object | 分析の対象: `login`、`id`(GitHub の node ID または `null`)、`is_authenticated_user` |
| `period` | object | `start`(含む)と `end`(含まない)。レポートのタイムゾーンの RFC 3339 形式 |
| `timezone` | string | 暦の境界に使った IANA タイムゾーン |
| `metrics` | [Summary](#summary) | 期間全体のまとめ |
| `snapshot` | [Snapshot](#snapshot) | 現在の状態の値 |
| `repositories` | [RepoSummary](#reposummary) の配列 | リポジトリごとのまとめ。自分の活動の多い順、次に名前の順 |
| `activity_breakdown` | [BucketSummary](#bucketsummary) の配列 | 区切りごとのまとめを時刻の順に並べたもの。コマンドが `weekly`、`monthly`、`yearly` でなければ、日ごとの区切りです |
| `granularity` | string | `day`、`week`、`month`、`year`。`daily`、`weekly`、`monthly`、`yearly` のときだけ入ります |
| `coverage` | [CoverageSummary](#coveragesummary) の配列 | データの種類ごとに1要素 |
| `synced_at` | タイムスタンプまたは `null` | このアカウントの最後の同期が終わった時刻 |
| `private_repositories_redacted` | boolean | private の名前を伏せたときに `true`(`--include-private` なしの JSON/CSV) |

コマンドに固有のフィールド:

| コマンド | フィールド |
|---|---|
| `prs` | `pull_requests`([PrRow](#prrow) の配列)、`intensity`([Intensity](#intensity))、`timing`([TimingStats](#timingstats)) |
| `reviews` | `reviews_submitted`、`reviews_received`([ReviewRow](#reviewrow) の配列) |
| `comments` | `comments`([CommentRow](#commentrow) の配列) |
| `issues` | `issues`([IssueRow](#issuerow) の配列) |

各件数の定義は[指標の定義](./metrics.md)にあります。

### Summary

| フィールド | 型 |
|---|---|
| `commits`, `merge_commits` | integer |
| `prs_created`, `prs_merged`, `merges_performed`, `merges_unknown_actor` | integer |
| `prs_closed_without_merge`, `prs_reopened` | integer |
| `reviews_submitted` | [ReviewCounts](#reviewcounts) |
| `reviewed_prs` | integer(ユニーク数) |
| `reviews_received` | [ReviewCounts](#reviewcounts) |
| `reviews_received_from_bots`, `review_dismissals_performed` | integer |
| `comments_written`, `comments_received` | [CommentCounts](#commentcounts) |
| `review_responses`, `pr_commits`, `force_pushes` | integer |
| `coordination_actions`, `metadata_actions` | integer |
| `issues_opened`, `issues_closed`, `issues_reopened` | integer |
| `active_prs`, `active_prs_all_participants` | integer(ユニーク数) |
| `pr_internal_events` | integer |
| `ci` | object: `passed`、`failed`、`pending`、`other`(integer)。活動には決して含めません |
| `active_days` | integer |
| `last_activity` | タイムスタンプまたは `null` |
| `diff_stats` | [DiffTotals](#difftotals) |

#### ReviewCounts

`total`、`approved`、`changes_requested`、`commented`、`dismissed_unknown_state`(integer)。

#### CommentCounts

`total`、`conversation`、`inline`、`review_summary`(integer)。`total` は3種類の合計です。

#### DiffTotals

数えた自分のコミットで追加・削除した行数です。[変更行数](./metrics.md#lines-changed)を参照してください。

| フィールド | 型 | 説明 |
|---|---|---|
| `commits` | integer | `commits` で数えたコミットのうち、マージコミットでないもの |
| `excluded` | integer | そのうち、変更がほかのコミットとして数えられるために行数の合計から除いたもの |
| `commits_with_stats` | integer | 残りの `commits - excluded` 件のうち、行数が分かっているもの |
| `additions`, `deletions` | integer | `commits_with_stats` 件のコミットだけの合計 |
| `coverage` | string | `no_commits`、`complete`、`partial`、`unavailable` |

`coverage` が `unavailable` のとき、`additions` と `deletions` は `0` になります。この `0` は「不明」の意味で、0 行という意味ではありません。合計を使う前に `coverage` を確かめてください。

```json
"diff_stats": {
  "commits": 42,
  "excluded": 6,
  "commits_with_stats": 30,
  "additions": 1830,
  "deletions": 612,
  "coverage": "partial"
}
```

### Snapshot

| フィールド | 説明 |
|---|---|
| `open_prs` | 自分のプルリクエストのうち、最後に同期した時点でオープンだったもの |
| `open_issues_authored` | 自分が作成した Issue のうち、現在オープンなもの |
| `threads` | 期間内に活動のあったプルリクエストのレビュースレッド: `total`、`open`、`resolved`、`outdated` |

### RepoSummary

| フィールド | 説明 |
|---|---|
| `repository_id` | リポジトリの GitHub node ID |
| `full_name` | `owner/name`(private リポジトリでは伏せます) |
| `visibility` | `public`、`private`、`internal`、`unknown` |
| `owned` | 自分が所有しているか([CLI リファレンス](./cli.md#global-options)を参照) |
| `accessible` | 同期中に GitHub がリポジトリへのアクセスを拒否した(HTTP 403 または 404)ときは `false`。キャッシュ済みのデータは残します |
| `summary` | このリポジトリの [Summary](#summary) |

### BucketSummary

| フィールド | 説明 |
|---|---|
| `bucket.key` | 暦の区切りの最初のローカルの日付(`YYYY-MM-DD`) |
| `bucket.label` | `2026-11-21`(日)、`2026-W47`(ISO 週)、`2026-11`(月) |
| `bucket.start`, `bucket.end` | 区切りの正確な UTC の範囲。期間に合わせて切り詰めます |
| `summary` | この区切りの [Summary](#summary) |

### CoverageSummary

| フィールド | 説明 |
|---|---|
| `kind` | `commits`、`pull_requests`、`issues` |
| `status` | 期間全体についての `complete`、`partial`、`not_available`、`not_yet_synced`、`unknown` |
| `scopes` | 何を取得したかを、人が読める説明で表したもの |
| `segments` | 期間を順に覆う `{start, end, status, reason}` の配列。`reason` は complete でない区間の理由で、それ以外は `null` |

レポート全体のカバレッジが `complete` になるのは、すべての要素が `complete` のときだけです。`--require-complete` はこれを使います。

### PrRow

| フィールド | 説明 |
|---|---|
| `pr_id` | プルリクエストの GitHub node ID |
| `repository` | `owner/name` |
| `number`, `title`, `url` | プルリクエストの番号、タイトル、URL |
| `author` | 作成者のログイン名(不明なときは `unknown`) |
| `state` | 現在の状態: `open`、`closed`、`merged` |
| `is_draft` | 現在のドラフトかどうか |
| `created_at`, `merged_at` | タイムスタンプ(マージされていなければ `merged_at` は `null`) |
| `events`, `events_by_subject` | 期間内に数えた CI 以外のイベントの数。全員の分 / 自分の分 |
| `commits`, `reviews`, `comments`, `participants` | 期間内の件数 |
| `discussion_participants` | 期間内に会話コメント、インラインコメント、レビューサマリーのコメントを書いた、既知のアカウントの数(重複なし) |
| `threads` | 期間内にインラインコメントのあったレビュースレッド |
| `last_activity` | 期間内で最も新しいイベントの時刻(コミットは選んだ `--commit-date` を使います) |
| `latest_ci` | head コミットの現在の CI の状態: `success`、`failure`、`pending`、`neutral`、`skipped`、`cancelled`、`unknown`、または `null` |
| `diff` | [PrDiff](#prdiff)。分からないときは `null` |

`--sort` の順(大きい順・新しい順)、次にリポジトリと番号の順に並びます。

#### PrDiff

最後に同期した時点での、プルリクエストの base と head の差分です。プルリクエストのスナップショットで、期間中に行った変更ではありません。[プルリクエストの変更行数](./metrics.md#pull-request-changes)を参照してください。

| フィールド | 説明 |
|---|---|
| `additions`, `deletions` | 追加した行数と削除した行数(integer) |
| `changed_files` | 変更したファイルの数 |
| `observed_at` | 差分を取得した時刻 |

### Intensity

`prs`(プルリクエストの数)と、`events_per_pr`、`reviews_per_pr`、`comments_per_pr`、`commits_per_pr`、`participants_per_pr`、`discussion_participants_per_pr`、`threads_per_pr`、`active_days_per_pr`。後者はそれぞれ [Stats](#stats) オブジェクトで、プルリクエストがないときは `null` です。

### TimingStats

`time_to_first_review`、`creation_to_merge`、`review_to_next_author_activity`、`first_to_last_activity`。それぞれ秒単位の [Stats](#stats) オブジェクトで、その値を持つプルリクエストがないときは `null` です。[ライフサイクルの所要時間](./metrics.md#details-lifecycle-timing-your-pull-requests)を参照してください。

#### Stats

`count`、`min`、`median`、`mean`、`p90`(nearest-rank 法による 90 パーセンタイル)、`max`。小数になることがあります。

### ReviewRow

| フィールド | 説明 |
|---|---|
| `event_id` | 変わらないイベントの識別子 |
| `submitted_at` | 提出した時刻 |
| `reviewer` | レビュアーのログイン名 |
| `state` | `APPROVED`、`CHANGES_REQUESTED`、`COMMENTED`、`DISMISSED`(元の状態が不明)、または `null` |
| `repository` | `owner/name` |
| `pr_id` | レビューしたプルリクエストの GitHub node ID、または `null` |
| `pr_number`, `pr_title` | プルリクエストの番号とタイトル(または `null`) |
| `url` | レビューへのリンク、または `null` |

`reviews_submitted` には、数えた自分のレビューが入ります。`reviews_received` には、自分のプルリクエストにほかの人が提出したレビューが入ります。どちらも時刻の順です。

### CommentRow

| フィールド | 説明 |
|---|---|
| `event_id` | 変わらないイベントの識別子 |
| `created_at` | 作成した時刻 |
| `author` | 作成者のログイン名 |
| `kind` | `conversation_comment`、`inline_comment`、`review_summary` |
| `written_by_subject` | 自分が書いたコメントなら `true` |
| `repository`, `pr_number` | コメントのある場所 |
| `pr_id` | プルリクエストの GitHub node ID、または `null` |
| `edited` | コメントが編集されたか |
| `url` | リンク、または `null` |

自分が書いたコメントと、自分のプルリクエストにほかの人が書いたコメントを、時刻の順に並べます。

### IssueRow

| フィールド | 説明 |
|---|---|
| `issue_id` | GitHub node ID |
| `repository`, `number`, `title`, `url` | Issue |
| `is_open` | 現在の状態 |
| `opened_in_period` | 期間内に自分が作成したか |
| `closed_in_period`, `reopened_in_period` | 期間内の自分によるクローズ / 再オープンのイベント |

リポジトリと番号の順に並びます。

## プルリクエストの詳細(`ghusage pr`) {#pull-request-detail-ghusage-pr}

| フィールド | 説明 |
|---|---|
| `schema_version`, `account`, `timezone` | レポートと同じ |
| `pr_id`, `repository`, `number`, `title`, `url` | プルリクエスト |
| `author` | 作成者のログイン名 |
| `state`, `is_draft` | 現在の状態 |
| `created_at`, `merged_at`, `closed_at` | タイムスタンプ(該当しないときは `null`) |
| `merged_by` | マージした人のログイン名。マージされていないか不明なときは `null` |
| `latest_ci` | head コミットの現在の CI の状態(文字列)、または `null` |
| `commits`, `removed_commits` | 現在プルリクエストに含まれるコミット / フォースプッシュで取り除かれたコミット |
| `reviews` | [ReviewCounts](#reviewcounts) |
| `comments` | [CommentCounts](#commentcounts) |
| `review_threads` | `total`、`open`、`resolved`、`outdated`(現在の状態) |
| `participants` | 数えた CI 以外のイベントがある既知のアカウントのログイン名。並べ替え済み |
| `discussion_participants` | 会話コメント、インラインコメント、レビューサマリーのコメントを書いた既知のアカウントのログイン名。並べ替え済み |
| `commit_authors` | `{author, commits}` の配列。現在プルリクエストに含まれるコミットをコミットの作成者ごとに数えたもので、多い順に並びます |
| `timing` | `time_to_first_review_secs`、`creation_to_merge_secs`、`review_to_next_author_activity_secs`、`first_to_last_activity_secs`(integer または `null`) |
| `diff` | 最後に同期した時点の [PrDiff](#prdiff)。分からないときは `null` |
| `timeline` | イベントの順に並べた [TimelineEntry](#timelineentry) の配列(`--event-type` で絞り込みます) |
| `coverage` | このプルリクエストのデータのカバレッジの状態 |
| `synced_at` | プルリクエストを最後に取得した時刻 |
| `private_repositories_redacted` | レポートと同じ |

### TimelineEntry

| フィールド | 説明 |
|---|---|
| `event_id` | 変わらないイベントの識別子 |
| `occurred_at` | 起きた時刻(コミットでは、push した時刻ではなくコミットの時刻) |
| `observed_at` | `ghusage` が取得した時刻 |
| `actor`, `actor_kind` | ログイン名(不明なときは `unknown`)と、`user`、`bot`、`organization`、`mannequin`、`unknown` のいずれか |
| `event_type` | [CLI リファレンス](./cli.md#pr)に挙げたイベントの種類のいずれか |
| `label` | 人が読めるイベント名 |
| `detail` | 補足の情報(レビューの状態、短い SHA、ラベル、変更前のタイトル、CI のチェックと結果、"thread reply only, not counted as a review"、"edited"、"via merge"、"no longer present" といった注記)、または `null` |
| `url` | リンク、または `null` |
| `removed_observed_at` | 項目がなくなったことを確認した時刻、または `null` |

同じ時刻のイベントは、プルリクエストの流れに沿った順(作成、コミット、フォースプッシュ、…、マージ、クローズ、再オープン)に並べます。

## コミット(`ghusage commit`) {#commit-ghusage-commit}

| フィールド | 説明 |
|---|---|
| `schema_version` | レポートと同じ |
| `repository`、`sha`、`url` | コミット(`url` は GitHub 上のページ。なければ `null`) |
| `message` | コミットメッセージの全文 |
| `author`、`committer` | `{name, login, date}`。git の名前、ひも付いた GitHub のログイン名、日時(それぞれ `null` のことがある) |
| `parents` | 親コミットの SHA |
| `is_merge` | 親コミットが 2 つ以上あるかどうか |
| `stats` | `files`、`additions`、`deletions`、`total` |
| `files` | `{filename, previous_filename, status, additions, deletions, changes, patch_available}` の配列。`--patch` を付けると、GitHub が diff を返したファイルには `patch`(unified diff)も付く |
| `files_truncated` | 一覧に載っていない変更ファイルがあるかどうか |
| `source` | `cache` か `github` |
| `fetched_at` | GitHub から取得した日時 |
| `private_repositories_redacted` | レポートと同じ |

`ghusage commit` に CSV 出力はありません。

## その他のコマンド {#other-commands}

### sync

`ghusage sync --format json`:

| フィールド | 説明 |
|---|---|
| `status` | `complete` または `partial` |
| `commits_found` | コミット検索とデフォルトブランチのコミット一覧で見つかったコミットの数(重複なし)。プルリクエストの中でだけ見つかったコミットは含みません |
| `pull_requests_found`, `pull_requests_fetched`, `pull_requests_unchanged` | 検索で見つかった / 取得した / 変更がないため取得を省いたプルリクエストの数 |
| `issues_found`, `issues_fetched`, `issues_unchanged` | 検索で見つかった / 取得した / 変更がないため取得を省いた Issue の数 |
| `failures` | 取得に失敗した、または一部しか取得できなかったプルリクエストや Issue の説明 |
| `coverage` | 書き込んだカバレッジの記録: `kind`、`start`、`end`、`status`、`scope`、`reason`、`recorded_at` |
| `api` | `requests`、`retries`、`rate_limit_waits`、`total_api_time_ms`、`bytes_received`(`--diff-stats` の行数のリクエストも含む) |
| `diff_stats` | `--diff-stats` を付けたときだけ入ります。`commits`(期間内の自分のマージでないコミットのうち、行数を合計に入れるもの)、`already_known`(キャッシュにすでにあり、取り直さなかったもの)、`fetched`(今回取得したもの)、`unavailable`(GitHub が返さず、不明のままのもの) |

### doctor

`{"ok": true, "checks": [{"ok": true, "message": "..."}]}`

### auth status

`authenticated`、`source`、`login`、`id`、`scopes`(配列または `null`)、`rate_limit`(`core_remaining`、`core_limit`、`core_reset`、`search_remaining`、`search_limit`、`graphql_remaining`、`graphql_limit`、または `null`)。

### cache status

`path`、`file_size_bytes`、`schema_version`、`account`、`repositories`、`pull_requests`、`issues`、`events`、`last_sync`(`started_at`、`finished_at`、`status`、`message`、または `null`)、`coverage_last_30_days`([CoverageSummary](#coveragesummary) の配列)。

## CSV

CSV の出力は、コマンドごとにヘッダー行付きの平らな表が1つです。不明な値は空のセルになり、ゼロは `0` と書きます。タイムスタンプは UTC の RFC 3339 形式です。真偽値は `true` / `false` です。自由記述のテキスト(タイトル、詳細)からは制御文字を取り除きます。また、先頭が `=`、`+`、`-`、`@` のときは前に `'` を付け、表計算ソフトが数式として評価しないようにします。

### overview, daily, weekly, monthly, yearly

暦の区切りごとに1行です(`overview` は日ごとの行を出力します)。

```text
period,start,end,commits,merge_commits,prs_created,prs_merged,merges_performed,prs_closed_without_merge,reviews_submitted,reviewed_prs,reviews_received,comments_written,comments_received,review_responses,pr_commits,issues_opened,issues_closed,active_prs,pr_internal_events,ci_passed,ci_failed,additions,deletions,line_count_commits,line_count_commits_known,line_count_commits_excluded,line_count_coverage,coverage
```

`period` は区切りのラベルです。`reviews_submitted` / `reviews_received` は合計、`comments_written` / `comments_received` は3種類すべての合計、`coverage` はその区切りのカバレッジをまとめたものです。行数の列は[下で](#line-count-columns)説明します。

### repos

```text
repository,visibility,owned,accessible,commits,prs_created,prs_merged,reviews_submitted,comments_written,issues_opened,active_prs,pr_internal_events,additions,deletions,line_count_commits,line_count_commits_known,line_count_commits_excluded,line_count_coverage,last_activity,period_coverage
```

#### 行数の列 {#line-count-columns}

| 列 | 説明 |
|---|---|
| `additions`, `deletions` | `diff_stats.additions` と `diff_stats.deletions`。行数が分かっているコミットがないとき(`unavailable`)は空欄。`partial` のときは、行数が分かっているコミットだけの合計 |
| `line_count_commits` | 行数を合計に入れるコミットの数:`diff_stats.commits - diff_stats.excluded` |
| `line_count_commits_known` | そのうち、行数が分かっているもの(`diff_stats.commits_with_stats`) |
| `line_count_commits_excluded` | ほかのコミットとして数えられるために除いたもの(`diff_stats.excluded`) |
| `line_count_coverage` | `no_commits`、`complete`、`partial`、`unavailable` |

`prs` の CSV には行数の列はありません。プルリクエストの差分は JSON で取得してください。

### prs

```text
repository,number,title,state,draft,author,created_at,merged_at,events,events_by_you,commits,reviews,comments,participants,discussion_participants,threads,last_activity,latest_ci,url,period_coverage
```

### reviews

```text
direction,submitted_at,reviewer,state,repository,pr_number,url,period_coverage
```

`direction` は `submitted` または `received` です。

### comments

```text
created_at,author,kind,written_by_you,repository,pr_number,edited,url,period_coverage
```

### issues

```text
repository,number,title,open,opened_in_period,closed_in_period,reopened_in_period,url,period_coverage
```

### pr

プルリクエストのタイムラインを、イベントごとに1行で出力します。

```text
occurred_at,actor,actor_kind,event_type,detail,removed_observed_at,url
```

`period_coverage` は、期間全体をまとめたカバレッジです。`sync`、`doctor`、`auth status`、`cache status`、`tui` には CSV 形式がなく、`--format csv` を指定すると終了コード 2 で失敗します。

```text
error: --format csv is not available for this command; use table or json
```

## private リポジトリ {#private-repositories}

`--include-private` を指定しないと、JSON と CSV の出力では、公開でない(private と internal の)リポジトリの情報を伏せます。数値はどちらでも同じです。

| 項目 | 置き換え後 |
|---|---|
| リポジトリ名 | `<owner>/repo-<8 hex characters>`。owner は、自分が所有するリポジトリでは自分のログイン名、それ以外では `private` です。後ろの部分はリポジトリ ID から作るので、実行するたびに変わることはありません |
| プルリクエストと Issue のタイトル | `[private]` |
| プルリクエストと Issue の URL | 空文字列 |
| レビュー、コメント、イベントの URL、リポジトリの URL | `null` |
| レビュースレッドのファイルパス、ラベル名とマイルストーン名、変更前のタイトル、CI のチェック名 | 削除 |
| カバレッジの理由と `sync` の失敗の説明に出てくるリポジトリ名 | `[private repository]`(公開だと分かっていないリポジトリすべて) |
| 公開だと分かっていないリポジトリの `ghusage commit` | リポジトリは `private/repo-<16 進数 8 文字>`、`url` は `null`、メッセージは `[private]`、ファイルパスは `[private]`。旧パス、diff、git の名前は出さない |

人のログイン名(自分の分もほかの人の分も)と GitHub node ID は伏せません。表形式の出力とターミナル UI は、自分にだけ表示されるものなので、常に実際の名前を表示します。
