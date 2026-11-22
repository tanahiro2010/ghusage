# 認証 {#authentication}

`ghusage` が GitHub API を呼び出すには、GitHub のトークンが必要です。`ghusage` はデータを読み取るだけで、GitHub に書き込むことはありません。

## トークンの取得元 {#where-the-token-comes-from}

認証情報は次の順に探します。最初に見つかったものを使います。

| 順序 | 取得元 | 補足 |
|---|---|---|
| 1 | `--token-file <PATH>` | 明示的に指定したファイル。黙って読み飛ばすことはありません(後述)。 |
| 2 | `GH_TOKEN` | 環境変数。空の値は無視します。 |
| 3 | `GITHUB_TOKEN` | 環境変数。空の値は無視します。 |
| 4 | `gh auth token` | GitHub CLI がインストールされていてログイン済みなら、シェルを介さずに直接実行します。 |

どれも見つからない場合、GitHub に接続する必要のあるコマンドは終了コード 3 で失敗します。

```text
error: no GitHub credentials found: set GH_TOKEN or GITHUB_TOKEN, log in with `gh auth login`, or pass --token-file
```

### トークンファイル {#token-files}

`--token-file` は、トークンを 1 つだけ書いたファイルを読み込みます(前後の空白や末尾の改行はあっても構いません)。トークンファイルを指定した場合、`ghusage` はそのファイルを使うか、失敗するかのどちらかです。ほかの取得元に切り替えることはしません。次のいずれかに当てはまるファイルは受け付けません(終了コード 3)。

- 存在しない、通常のファイルではない、または読み込めない。
- 空である、64 KiB を超える、または 2 語以上を含む。
- Unix で、グループや他のユーザーが読み書きできる。

```sh
umask 077
printf '%s\n' "$TOKEN" > ~/.config/ghusage-token
chmod 600 ~/.config/ghusage-token
ghusage --token-file ~/.config/ghusage-token auth status
```

```text
$ chmod 644 token.txt
$ ghusage --token-file token.txt auth status
error: token file token.txt must not be readable by other users (use chmod 600)
```

パーミッションの確認は Unix 系のシステムでだけ行います。

## 認証情報の確認 {#checking-credentials}

```sh
ghusage auth status
```

このコマンドは、ログイン名、トークンの取得元、トークンの OAuth スコープ(GitHub が返した場合)、現在のレート制限を表示します。`ghusage doctor` は、同じ確認をキャッシュの確認とあわせて行います。

GitHub がトークンを拒否した場合(HTTP 401)、コマンドは終了コード 3 で失敗します。

```text
error: authentication failed: GitHub rejected the credentials (HTTP 401)
```

## どのトークンを使うか {#which-token-to-use}

`ghusage` が見られるのは、トークンで見られる範囲だけです。

- **公開データだけを扱う場合**: 有効なトークンならどれでも使えます。プライベートリポジトリを取得も表示もしないようにするには `--public-only` を使います。
- **プライベートリポジトリ**: `repo` スコープを持つ classic の個人アクセストークンか、ログイン済みの GitHub CLI のトークンを使うと、読み取り権限のあるプライベートリポジトリにアクセスできます。
- **SAML シングルサインオンを使う組織**: トークンをその組織向けに承認しておく必要があります。承認していないと、その組織のリポジトリは返されないか、アクセスできないものとして報告されます。
- **Fine-grained トークン**では、許可されたリポジトリしか見えません。検索結果やほかのリポジトリでの活動は見えません。

## ほかのユーザーの分析 {#analyzing-another-user}

```sh
ghusage overview --user octocat
```

`--user` を使うと、ほかのアカウントの活動を分析できます。対象になるのは、自分のトークンで見られる範囲だけです。ほかのユーザーについての所有の判定(`--ownership owned`)は、リポジトリのオーナーだけで行います。自分のトークンの管理者権限から、そのユーザーの権限は分からないためです。

## 公開リポジトリのみのモード {#public-only-mode}

```sh
ghusage sync --public-only
ghusage overview --public-only
```

`--public-only` を指定すると、検索の対象を公開リポジトリに限り、公開されていないリポジトリのデータはレポートから除きます。

`--public-only` を付けて同期した範囲は、公開リポジトリのみのモードでしか同期済みとして扱いません。あとで `--public-only` を付けずに実行すると、その範囲はまだ同期していないものとして扱われ、今度はプライベートリポジトリも含めて取得し直します。通常のモードで同期した範囲は、公開リポジトリのみのモードでも同期済みとして扱います。その場合、プライベートのデータはレポートから除かれるだけです。

## GitHub Enterprise Server {#github-enterprise-server}

`GHUSAGE_API_URL` に、サーバーの REST API のベース URL を設定します。GitHub Enterprise Server の URL のように `/api/v3` で終わる場合、GraphQL のエンドポイントは同じホストの `/api/graphql` になります。そうでなければ `<GHUSAGE_API_URL>/graphql` になります。別の URL を使うには `GHUSAGE_GRAPHQL_URL` を設定します。

```sh
export GHUSAGE_API_URL=https://github.example.com/api/v3
# GraphQL: https://github.example.com/api/graphql (derived)
export GH_TOKEN=...   # a token for that server
ghusage sync
```

GitHub Enterprise Server での動作はテストしていません。`gh auth token` は GitHub CLI の既定のホストのトークンを返すので、`GH_TOKEN` を設定するか、`--token-file` を明示的に指定してください。また、`ghusage pr` とターミナル UI からリンクを開く機能は、現在 `github.com` の URL にしか対応していません。

## トークンの安全性 {#token-safety}

- トークンをキャッシュのデータベース、ログ、出力に書き込むことはありません。
- トークンを送る先は、設定された API エンドポイントだけです。別のホストを指すページネーションのリンクは拒否します。
- `ghusage cache clear` は認証情報に触れません。
