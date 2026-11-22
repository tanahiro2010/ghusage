# 指標の定義 {#metric-definitions}

このページでは、`ghusage` が出力するすべての数値を定義します。表形式の出力、JSON、CSV、ターミナル UI はどれも同じコードで計算するので、ここでの定義はすべてに当てはまります。JSON のフィールド名は `code` の形で示します。

## 基本のルール {#basic-rules}

### イベント {#events}

`ghusage` が数えるものは、すべて**イベント**です。コミット、プルリクエストのオープン、レビューの提出、コメント、ラベルの追加、チェック実行の完了などがイベントに当たります。各イベントは変わらない識別子を持つので、同じイベントを 2 回取得しても、GitHub の別々の API から取得しても 1 回として数えます。データを取得した順序によって結果が変わることもありません。

### 「自分」の範囲 {#you}

「自分」とは、分析の対象になっているアカウントです。認証したユーザー、または `--user` で指定したアカウントを指します。イベントのアクターがそのアカウントであれば、そのイベントは自分のものです。両方のアカウント ID が分かっている場合は ID だけを比べるので、名前を変えたログインや、他人が再利用したログインを自分と取り違えることはありません。

- **ボット**は、GitHub 上の種別が `Bot` のアカウント、またはログインが `[bot]` で終わるアカウントです。ボットの活動は自分の活動に含めません。自分が受けた活動の件数と、全員の活動の件数には含めます。
- **不明なアクター**(削除されたアカウントなど)は、自分として数えません。

### 数えるイベントの条件 {#which-events-count}

イベントは、次の条件をすべて満たすときに、その期間の件数に入ります。

1. イベントの時刻が期間内にある。期間は半開区間で、開始 ≤ 時刻 < 終了 です([期間とタイムゾーン](./time.md)を参照)。
2. リポジトリがフィルター(`--repo`、`--owner`、`--visibility`、`--ownership`、`--public-only`)に合う。
3. 削除されたことを確認していない([編集と削除](#edits-and-deletions)を参照)。
4. 保留中(下書きで未提出)のレビューではなく、[スレッドへの返信だけのレビュー](#thread-reply-only-reviews)でもない。

イベントの時刻は、GitHub 上でそれが起きた時刻です(`created_at`、`submitted_at`、タイムラインイベントの時刻)。コミットでは、既定で**コミッター日時**を使います。`--commit-date author` を指定すると、作成者の日時(author date)を使います。コミットの時刻はコミットを作った時刻で、プッシュした時刻ではありません。GitHub はプッシュの時刻を提供していないためです。

### 活動と CI は別に扱う {#activity-not-ci}

チェック実行とコミットステータス(CI)は**自動処理**です。誰の活動としても数えません。アクティブな日、アクティブな PR、PR 内のイベント、PR ごとのイベント数、参加者、所要時間のいずれにも含めません。CI の結果は別に示します。

### 期間の件数と現在の状態 {#period-counts-versus-current-state}

ほとんどの数値は、期間内のイベントを数えたものです。一部の数値は**スナップショット**で、期間に関係なく、最後に同期した時点の現在の状態を表します。オープン中のプルリクエスト、自分が作成したオープン中の Issue、レビュースレッドの解決状況がこれに当たります。表形式の出力では「(current)」または「(now)」が付き、JSON では `snapshot` の下に入ります。

### 合計と内訳 {#totals-and-breakdowns}

`daily`、`weekly`、`monthly`、`yearly` は、期間を暦の単位に区切ります。区切りは期間をすき間も重なりもなく覆います。各区切りのイベント数を足すと、期間の合計と一致します。ユニーク数は足しても合計と一致しません。たとえば、2 日に分けてレビューしたプルリクエストは、その 2 日それぞれで 1 件と数えますが、合計では 1 件です。ユニーク数に当たるのは `reviewed_prs`、`active_prs`、`active_prs_all_participants` です。

## 集計の指標 {#summary-metrics}

ここで説明する指標は `overview` に表示されます。JSON のレポートではどれも `metrics` の下に入り、区切りごとの値は `activity_breakdown[].summary`、リポジトリごとの値は `repositories[].summary` に入ります。

### コミット {#commits}

**Commits**(`commits`):期間内に自分が作成したコミットの数です。リポジトリと SHA の組み合わせで区別します。

- 取得元は 2 つあります。1 つは、自分がプッシュしたリポジトリや貢献したリポジトリのデフォルトブランチにある、作成者が自分のアカウントのコミットです(各リポジトリのコミット一覧と GitHub のコミット検索から見つけます)。もう 1 つは、`ghusage` が同期したプルリクエストの中にある自分のコミットです。複数の方法で見つかったコミットも 1 回だけ数えます。詳しくは[コミットの見つけ方](../guide/syncing.md#how-commits-are-found)を参照してください。
- GitHub がコミットの作成者を自分のアカウントに結び付けている場合に、そのコミットを自分のものとします(通常はコミットのメールアドレスで結び付けます)。作成者のメールアドレスが自分のアカウントに結び付いていないコミットは、自分のものになりません。
- 同じ SHA が 2 つのリポジトリにある場合(フォークとそのフォーク元など)は、2 件のコミットとして数えます。
- フォースプッシュによって後でプルリクエストから外れたコミットも数えます。自分が作ったコミットであることに変わりはないからです。
- 時刻はコミッター日時(既定)または作成者の日時(`--commit-date author`)で決めます。

**Merge commits**(`merge_commits`):上のコミットのうち、親が 2 つ以上あるものの数です。`(N merge)` と表示します。

これらのコミットで追加・削除した行数については、[変更行数](#lines-changed)で説明します。

### プルリクエスト {#pull-requests}

**PRs created**(`prs_created`):期間内に自分が作成したプルリクエストです(作成時刻で数えます)。

**PRs merged (yours)**(`prs_merged`):自分のプルリクエストのうち、期間内にマージされたものです。誰がマージしたかは問わず、マージの時刻で数えます。期間より前に作成して期間内にマージされたプルリクエストは数えますが、期間内に作成して期間後にマージされたものは数えません。

**Merges you performed**(`merges_performed`):期間内のマージのうち、自分がマージを実行したものです。プルリクエストの作成者は問いません。自分のプルリクエストを自分でマージした場合は、ここにも「PRs merged (yours)」にも入ります。

**Merged by unknown**(`merges_unknown_actor`):マージされた自分のプルリクエストのうち、マージした人が分からないもの(GitHub が報告しなかったもの)の数です。表形式の出力では、0 でないときだけ表示します。

**PRs closed without merge**(`prs_closed_without_merge`):期間内に自分のプルリクエストで起きたクローズイベントのうち、マージに伴うものではないと分かっているものです。クローズした人は問いません。プルリクエストがマージ済みで、マージから 60 秒以内にクローズされた場合は、そのクローズをマージに伴うものとみなします。クローズ、再オープン、再クローズと続いたプルリクエストは 2 回と数えます。

**PRs reopened**(`prs_reopened`、JSON と TUI のみ):期間内に自分のプルリクエストで起きた再オープンイベントです。再オープンした人は問いません。

**Open PRs (current)**(`snapshot.open_prs`):キャッシュにある自分のプルリクエストのうち、最後に同期した時点でオープンだったものです。期間内の件数ではなく、現在の状態です。リポジトリのフィルターは適用されます。

### レビュー {#reviews}

GitHub のレビューには、approved(承認)、changes requested(変更の要求)、commented(コメント)のいずれかの状態があります。

**Reviews submitted**(`reviews_submitted`):期間内に自分が提出したレビューです。誰のプルリクエストかは問いません。

- `total`:数えた提出の総数です。1 つのプルリクエストに 3 回レビューを提出すれば 3 件です。
- `approved`、`changes_requested`、`commented`:状態ごとの件数です。
- `dismissed_unknown_state`:却下(dismiss)されたレビューのうち、元の状態を判別できなかったものです。却下される前の状態を GitHub が報告している場合は、その状態として数えます。
- 保留中(未提出)のレビューは決して数えません。

**Reviewed PRs**(`reviewed_prs`):期間内に、数える対象のレビューを 1 回以上提出したプルリクエストの数です。ユニーク数です。`on N PRs` と表示します。

**Reviews received**(`reviews_received`):期間内に、自分以外(人、ボット、不明なアクター)が自分のプルリクエストに提出したレビューです。状態ごとの内訳も同じように示します。

**From bots**(`reviews_received_from_bots`):受けたレビューのうち、ボットが提出したものの数です。

**Review dismissals performed**(`review_dismissals_performed`、JSON のみ):期間内に自分が却下したレビューです。

#### スレッドへの返信だけのレビュー {#thread-reply-only-reviews}

既存のレビュースレッドに返信すると、GitHub はその返信を、状態が「commented」で本文のないレビューとして記録します。これを数えるとレビューの件数が実際より増えてしまいます。そのため、状態が commented のレビューで、本文(サマリー)がなく、含まれるインラインコメントがすべて既存のスレッドへの返信である場合は、レビューの提出として**数えません**。返信そのものは、インラインコメントとして数えます。`ghusage pr --timeline` では、このようなレビューに「thread reply only, not counted as a review」と表示します。

#### レビューの本文(サマリー) {#review-summaries}

提出したレビューの本文を、**レビューの本文**(サマリー)と呼びます。これはコメント(種別 `review_summary`)として数え、レビューの件数には加えません。本文が空か空白だけのレビューには、本文(サマリー)がないものとします。

### コメント {#comments}

コメントには 3 つの種別があります。

| 種別 | JSON のキー | 内容 |
|---|---|---|
| 会話コメント | `conversation` | プルリクエストの会話欄に書くコメント(コードには付かない) |
| インラインコメント | `inline` | コードに付けるレビューコメント。レビュースレッドの中にあり、返信も含む |
| レビューの本文(サマリー) | `review_summary` | 提出したレビューの本文 |

**Comments written**(`comments_written`):期間内に自分が書いたコメントを、種別ごとに数えたものです。どのプルリクエストかは問いません。合計(JSON の `total`)は 3 種別の和です。

**Comments received**(`comments_received`):期間内に、自分以外(ボットを含む)が自分のプルリクエストに書いたコメントを、種別ごとに数えたものです。

Issue のコメントは収集しません。コメントの指標はプルリクエストだけが対象です。コメントの編集は活動として数えません。

**Review thread replies**(`review_responses`):自分のプルリクエストのレビュースレッドのうち、ほかの人(既知のアカウント)が始めたスレッドに自分が書いたインラインの返信です。レビューでの指摘に対する自分の回答に当たります。レビューの後に追加でプッシュしたコミットは、ここには数えません。

### プルリクエストでの開発 {#pull-request-development}

**PR commits**(`pr_commits`):自分が作成したコミットのうち、現在いずれかのプルリクエストに含まれているものの数です。コミットの時刻が期間内にあるものを数えます。2 つのプルリクエストに含まれるコミットは、それぞれで 1 回ずつ数えます。フォースプッシュでプルリクエストから外れたコミットは含めません。

**Force pushes**(`force_pushes`):期間内に自分がプルリクエストのブランチに行ったフォースプッシュです。プルリクエストのタイムラインに記録されたものを数えます。

**Coordination actions**(`coordination_actions`、JSON のみ):自分が行ったレビューの依頼、レビュー依頼の取り消し、担当者の割り当て、割り当ての解除です。

**Metadata actions**(`metadata_actions`、JSON のみ):自分が行ったラベルの追加と削除、マイルストーンの設定と解除、タイトルの変更です。

### Issue {#issues}

**Issues**(`issues_opened`、`issues_closed`、`issues_reopened`):期間内に自分が作成した Issue、自分が行ったクローズイベント、自分が行った再オープンイベントです。同じ Issue を 2 回クローズすれば 2 回と数えます。

**Open issues you authored (current)**(`snapshot.open_issues_authored`、`issues` で表示):自分が作成した Issue のうち、現在オープンのものです。

### 活動 {#activity}

**Active PRs**(`active_prs`):期間内に、自分のイベントが 1 つ以上あるプルリクエストの数です。イベントの種別は CI 以外なら問いません(コミット、レビュー、コメント、ラベルなど)。ユニーク数です。

**With anyone's activity**(`active_prs_all_participants`):同期したプルリクエストのうち、期間内に誰か(自分、ほかの人、ボット)の CI 以外のイベントが 1 つ以上あるものの数です。ユニーク数です。`ghusage` が知っているのは、自分が関わっているために同期したプルリクエストだけです。

**PR internal events**(`pr_internal_events`):期間内に、プルリクエストの中で起きた自分のイベントの数です。プルリクエストのコミット、ライフサイクルのイベント(オープン、クローズ、再オープン、マージ、下書き状態の変更)、フォースプッシュ、数える対象のレビュー、却下、3 種別すべてのコメント、調整の操作、メタデータの操作が入ります。CI は含めません。デフォルトブランチで見つけたコミットと Issue のイベントは、プルリクエストのイベントではありません。

**Active days**(`active_days`):数える対象になる CI 以外のイベント(コミット、プルリクエストのイベント、Issue のイベント)が自分に 1 つ以上ある日の数です。日付は、レポートのタイムゾーンでの暦の日付で、重複なく数えます。

**Last activity**(`last_activity`、JSON と CSV):期間内で、数える対象になる CI 以外の自分のイベントのうち、最も新しいものの時刻です。コミットでは、選んだコミットの日時(`--commit-date`)を使います。

### CI の結果 {#ci-results}

**CI results (not counted as activity)**(`ci`):同期したプルリクエストのチェック実行とコミットステータスのうち、時刻が期間内にあるものを、結果ごとに数えたものです。誰が起動したかは問いません。

| フィールド | 結果 |
|---|---|
| `passed` | success |
| `failed` | failure, timed out, startup failure, action required, error |
| `pending` | queued, in progress, waiting, requested, pending |
| `other` | neutral, skipped, stale, cancelled, unknown |

実行はすべて数えます。失敗したチェックを再実行して成功した場合は、`failed` に 1 回、`passed` に 1 回入ります。

同期では、その時点での各プルリクエストのヘッドコミットについて CI の結果を読みます。新しいコミットがプッシュされても、以前のヘッドコミットについて保存済みの結果は残すので、以前のヘッドでの失敗も数に入ります。一方、ヘッドだった間に一度も同期しなかったヘッドコミットの CI は、取得できません。

### レビュースレッド(現在の状態) {#review-threads-current}

`snapshot.threads` は、期間内に誰かの CI 以外の活動があったプルリクエストについて、そのレビュースレッドを現在の状態ごとに数えたものです。`total`、`open`(未解決)、`resolved`(解決済み)、`outdated`(参照しているコードが変わったもの。open や resolved と重なる)があります。スレッドがいつ解決されたかは分かりません。

## 変更行数 {#lines-changed}

`ghusage` は、自分のコミットで追加・削除した行数(JSON の `diff_stats`)を出します。また、それとは別に、各プルリクエストの現在の差分も出します。どちらも GitHub が返す行数を使います。行数がいつ分かるかは、[行数の取得](../guide/syncing.md#line-counts)を参照してください。

::: warning 行数の読み方に注意してください
行数は、生成されたファイル、整形だけの変更、依存関係の更新(ロックファイルや同梱したコード)、ファイルの移動や名前の変更で大きく変わります。小さく丁寧な修正と、大きな機械的な変更とでは、桁が違うこともあります。行数は仕事の量や質を測るものではありません。点数やランキングにしたり、人と人を比べたりするために使わないでください。追加した行数と削除した行数は、足し合わせずに分けて見てください。
:::

### 自分のコミットの変更行数 {#lines-changed-by-your-commits}

**Lines changed (your commits)**(`diff_stats`):[Commits](#commits) で数えたコミットから、マージコミットと、[下で説明する除外](#commits-counted-through-other-commits)の対象を除いたものについて、追加した行数と削除した行数を合計したものです。

- 対象のコミットは `commits` と同じです。取得元、期間の区切り(コミッター日時、`--commit-date author` なら作成者の日時)、フィルター、重複の除き方はすべて同じです。デフォルトブランチとプルリクエストの両方で見つかったコミットは 1 回だけ数えます。
- **マージコミットは除きます。** GitHub はマージコミットの行数を first parent との差分で返すので、マージしたブランチの変更と重なります。その変更は、マージされた側のコミットですでに数えています。
- 行数が分からないコミットを 0 行として扱うことはありません。合計は行数が分かっているコミットだけを対象にし、それが何件かを `coverage` で示します。
- 日・週・月・年ごと、リポジトリごとの値も同じ定義なので、足すと期間全体の値と一致します。

| フィールド | 意味 |
|---|---|
| `commits` | `commits` で数えたコミットのうち、マージコミットでないもの |
| `excluded` | そのうち、変更がほかのコミットとして数えられるために除いたもの(下を参照) |
| `commits_with_stats` | 残りの `commits - excluded` 件のうち、行数が分かっているもの |
| `additions`, `deletions` | 追加した行数と削除した行数。`commits_with_stats` 件のコミットだけの合計 |
| `coverage` | `no_commits`:数える対象がない(コミットがない、またはすべて除外)。`complete`:残りのコミットすべての行数が分かっている。`partial`:一部だけ分かっているので、合計は下限。`unavailable`:どのコミットの行数も分からないので、合計は不明(0 ではない) |

コミット数(`commits`)の定義は、これによって変わりません。

### ほかのコミットとして数えられるコミット {#commits-counted-through-other-commits}

コミットの変更が、別のコミットとしてデフォルトブランチに届くことがあります。

- **squash** でマージしたプルリクエストからは、全コミットの変更をまとめた新しいコミットが 1 つ作られます。
- **rebase** でマージしたプルリクエストからは、各コミットの新しいコピーが作られます。
- **フォースプッシュ**でプルリクエストから外れたコミットは、たいてい書き直したコミットに置き換わっています。

元のコミットとデフォルトブランチに届いたコミットの両方を数えると、同じ変更を 2 回数えることになります。そこで、同期したプルリクエストに現れるコミットは、行数の合計から除きます(`excluded`)。ただし、現れ方のうち少なくとも 1 つが、今もそのプルリクエストに含まれていて、かつそのプルリクエストが次のどちらかに当たる場合は除きません。

- オープン中、またはマージせずにクローズされた
- マージコミット(親が 2 つのコミット)でマージされた

つまり、squash や rebase(またはマージ方法が分からない方法)でマージされたプルリクエストにしかないコミットと、フォースプッシュでプルリクエストから外れたコミットを除きます。同期したプルリクエストに一度も現れないコミットは、常に数えます。

マージ方法は、プルリクエストのマージコミットの親の数から判断します。同期のときに、プルリクエストと一緒に記録します。`ghusage` がマージ方法を記録するようになる前に同期したマージ済みのプルリクエストは、マージ方法が分からないので、そのコミットは除外されます。その期間について `ghusage sync --full` を実行すると、マージ方法を記録し直せます。

このルールは暫定のもので、今後のバージョンで見直すことがあります。うまく扱えない場合については、[既知の制約](./limitations.md#line-counts)を参照してください。

### プルリクエストの変更行数 {#pull-request-changes}

**Changes**([`prs`](#pull-request-list) と [`pr`](#pull-request-detail) の `diff`):最後に同期した時点での、プルリクエストの base と head の差分です。追加した行数(`additions`)、削除した行数(`deletions`)、変更したファイルの数(`changed_files`)、観測した時刻(`observed_at`)からなります。

- プルリクエストの**スナップショット**で、期間中に行った変更ではありません。プッシュ、フォースプッシュ、base ブランチの更新のたびに変わります。
- 自分のコミットの変更行数の合計には足しません。両者は別のものを表していて、足すと同じ変更を 2 回数えることになります。
- 分からないとき(古いバージョンで同期したプルリクエストなど)は `null` で、表では `?` や「unknown」と表示します。

## 表のラベル {#table-labels}

| `overview` のラベル | JSON のフィールド |
|---|---|
| Commits | `commits` (`merge_commits`) |
| PRs created | `prs_created` |
| PRs merged (yours) | `prs_merged` |
| Merges you performed | `merges_performed` (`merges_unknown_actor`) |
| PRs closed without merge | `prs_closed_without_merge` |
| Open PRs (current) | `snapshot.open_prs` |
| Reviews submitted | `reviews_submitted`, `reviewed_prs` |
| Reviews received | `reviews_received`, `reviews_received_from_bots` |
| Comments written | `comments_written` |
| Comments received | `comments_received` |
| Review thread replies | `review_responses` |
| PR commits | `pr_commits` |
| Force pushes | `force_pushes` |
| Issues | `issues_opened`, `issues_closed`, `issues_reopened` |
| Active PRs | `active_prs`, `active_prs_all_participants` |
| PR internal events | `pr_internal_events` |
| CI results (not counted as activity) | `ci` |
| Active days | `active_days` |
| Lines changed (your commits) | `diff_stats` |

「Lines changed (your commits)」の表示は、`diff_stats.coverage` によって次のように変わります。complete なら `+A -D in N commits`、partial なら ``+A -D in K of N commits; the rest are unknown (run `ghusage sync --diff-stats`)``、unavailable なら ``unknown for N commits (run `ghusage sync --diff-stats`)`` です。N は `commits - excluded` です。除外したコミットがあるときは、`; E commits left out as counted through other commits (squash or rebase merges, force pushes)` が後ろに付きます。

`daily`、`weekly`、`monthly`、`yearly` の列は次のとおりです。Commits(`commits`)、PRs(`prs_created`)、Merged(`prs_merged`)、Reviews(`reviews_submitted.total`)、Received(`reviews_received.total`)、Comments(書いたコメント、全種別)、PR commits(`pr_commits`)、Issues(`issues_opened`)、Active PRs(`active_prs`)、+Lines(`diff_stats.additions`)、-Lines(`diff_stats.deletions`)。

`repos` の列は次のとおりで、どれもそのリポジトリでの値です。Commits、PRs(作成)、Merged(自分のもの)、Reviews(提出)、Comments(書いたもの)、Issues(作成)、Active PRs、PR events(`pr_internal_events`)、+Lines、-Lines、Last activity。リポジトリは、そこでの自分の活動量(コミット + PR 内のイベント + Issue のイベント)の順に並べ、同じなら名前の順に並べます。

+Lines と -Lines の列(表形式と Markdown の出力)では、`~N` は一部のコミットだけの合計(`partial`)、`?` は行数が分かっているコミットがないこと(`unavailable`)を表します。これらの印が出たときは、表の下に注記を出します。

## プルリクエストの一覧 {#pull-request-list}

`ghusage prs` は、期間内に CI 以外のイベントが 1 つ以上あるプルリクエストを一覧にします。既定では、そのイベントの少なくとも 1 つが自分のものであるプルリクエストだけを出します。`--all-participants` を付けると、ほかの人だけが活動したプルリクエストも出します。どの列も、期間内のイベントだけを数えます。

| 列 | JSON のフィールド | 定義 |
|---|---|---|
| State | `state`, `is_draft` | 現在の状態:`open`、`draft`、`merged`、`closed`(マージせずにクローズ) |
| Author | `author` | プルリクエストの作成者のログイン |
| Events | `events` | 数える対象になる CI 以外のイベント。誰のものでも数える(ボットを含む) |
| Yours | `events_by_subject` | そのイベントのうち、自分のもの |
| Commits | `commits` | プルリクエスト内のコミットのうち、コミットの時刻が期間内にあるもの(作成者は問わない)。重複なく数える |
| Reviews | `reviews` | 数える対象になるレビューの提出。誰のものでも数える |
| Comments | `comments` | 3 種別すべてのコメント。誰のものでも数える |
| People | `participants` | イベントが 1 つ以上ある既知のアカウント。重複なく数える |
| Discussion | `discussion_participants` | 会話コメント、インラインコメント、レビューの本文(サマリー)のいずれかを書いた既知のアカウント。重複なく数える(ラベル、コミット、レビューの依頼だけでは数えない) |
| Threads | `threads` | 期間内にインラインコメントがあったレビュースレッド |
| Last activity | `last_activity` | 上のイベントのうち最も新しいものの時刻 |
| CI | `latest_ci` | ヘッドコミットの現在の CI の総合状態(スナップショット) |
| Changes | `diff` | `+A -D`:最後に同期した時点での、プルリクエストの base と head の差分(スナップショットで、期間内の変更ではない。分からないときは `?`)。[プルリクエストの変更行数](#pull-request-changes)を参照 |

### `--details`:プルリクエストごとの活動 {#details-activity-per-pull-request}

一覧に出したプルリクエスト(上と同じ参加の条件)について、記述統計を出します。対象は、プルリクエストごとのイベント、レビュー、コメント、コミット、参加者、議論の参加者、レビュースレッド、アクティブな日です。アクティブな日は、期間内に CI 以外のイベントがあったローカルの日付を重複なく数えたものです。それぞれについて、`ghusage` は中央値、平均、90 パーセンタイル(最近傍順位法)、最大値、プルリクエストの数(`n`)を出します。JSON には最小値も入ります。

### `--details`:ライフサイクルの所要時間(自分のプルリクエスト) {#details-lifecycle-timing-your-pull-requests}

期間内に誰かの活動があった**自分の**プルリクエストについて計算します。期間内のイベントだけでなく、そのプルリクエストについて分かっていることをすべて使います。表では時間(hours)、JSON では秒で示します。

| 所要時間 | 定義 |
|---|---|
| Time to first review | 作成から、作成者以外(ボットを含む)が提出した、数える対象になる最初のレビューまで |
| Creation to merge | マージされたプルリクエストについて、作成からマージまで |
| Review to your next activity | その最初のレビューから、作成者の次に観測したイベント(種別は問わない)まで。これは**レビューへの対応とは限りません**。作成者がそのプルリクエストで次に行ったことを示すだけです |
| First to last activity | プルリクエストで観測した CI 以外の最初のイベントから、最後のイベントまで |

::: warning 品質のスコアではありません
活動の集中度と所要時間の値は、活動がどう分布しているかを示す記述統計です。品質、技能、生産性を測るものではなく、人と人を比べるために使うべきではありません。
:::

## プルリクエストの詳細 {#pull-request-detail}

`ghusage pr <URL>` は、期間ではなく、1 つのプルリクエストの作成から現在までの全体を対象にします。

| フィールド | 定義 |
|---|---|
| State (current) | `open`、`open (draft)`、`merged`、`closed` |
| Merged | マージの時刻とマージした人。GitHub が報告しなかった場合は「merger unknown」 |
| Commits | 現在プルリクエストに含まれるコミット。重複なく数える。「N no longer in the PR」は、以前に観測したコミットのうち、フォースプッシュで外れたものの数 |
| Reviews | 数える対象になるレビューの提出を状態ごとに数えたもの。誰のものでも数える |
| Comments | コメントを種別ごとに数えたもの。誰のものでも数える |
| Review threads | 現在の状態:合計、未解決、解決済み、outdated |
| Latest CI | ヘッドコミットの現在の CI の総合状態 |
| Changes (current snapshot) | `+A -D in F files (base to head as of <時刻>)`:最後に同期した時点でのプルリクエストの差分。[プルリクエストの変更行数](#pull-request-changes)を参照 |
| Participants | 数える対象になる CI 以外のイベントが 1 つ以上ある既知のアカウント |
| Time to first review, Creation to merge | 上の定義のとおり |

タイムライン(`--timeline`、JSON、CSV)には、保存したイベントをすべて並べます。数えないイベントも含みます(保留中のレビューはそもそも保存しません。削除された項目には「no longer present」と表示し、スレッドへの返信だけのレビューにも印を付けます)。PR のコミットは「Commit (committed)」と表示します。時刻が、プルリクエストに追加された時刻ではなくコミットの時刻だからです。

## 編集と削除 {#edits-and-deletions}

- コメントやレビューの編集は、活動として数えません。分かるのは最新の内容だけで、編集されたコメントには `edited` の印が付きます。
- 同期でプルリクエストを完全に取得し直したときに、以前に観測したコメント、レビュー、コミット、タイムラインイベントがなくなっていた場合は、削除を観測した時刻とともに削除済みの印を付け、件数から除きます。実際にいつ削除されたかは分かりません。存在しなくなったレビュースレッドは、スレッドの件数から外します。
- `ghusage` が最初に観測する前に削除された項目は、まったく分かりません。
