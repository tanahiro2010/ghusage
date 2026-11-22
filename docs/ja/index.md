---
layout: home

hero:
  name: ghusage
  text: GitHub で実際に何をしたのかを把握する。
  tagline: GitHub での活動を、プルリクエストの中で起きたことまで含めて分析する CLI とターミナル UI です。
  image:
    src: /overview.webp
    alt: ターミナルでの ghusage overview の出力
  actions:
    - theme: brand
      text: はじめる
      link: /ja/guide/getting-started
    - theme: alt
      text: 指標の定義
      link: /ja/reference/metrics
    - theme: alt
      text: GitHub で見る
      link: https://github.com/tanahiro2010/ghusage

features:
  - title: プルリクエストを過程として捉える
    details: コミット、レビューとその状態、インラインコメントと会話コメントの区別、レビュースレッド、フォースプッシュ、そして各プルリクエストのタイムライン全体を扱います。
  - title: 自分、ほかの人、ボットを分けて数える
    details: 書いたコメントと受けたコメント、送ったレビューと受けたレビュー、自分が行ったマージと自分のプルリクエストがマージされたことを区別します。CI の結果を自分の活動として数えることはありません。
  - title: 定義が書かれた正確な数値
    details: どの指標にも定義を文書で示しています。ユニーク数にはそう明記し、現在の状態を表す値には印を付けます。統計は記述のためのもので、評価のための点数ではありません。
  - title: 抜けを隠さない
    details: どのレポートにもカバレッジを表示します。一部しか取得できていないデータや欠けているデータはそのまま報告し、--require-complete を付ければ、不完全な数値を黙って使わずにスクリプトを失敗させられます。
  - title: ローカルで動き、オフラインでも使える
    details: データはローカルの SQLite キャッシュに保存し、差分で同期します。レポートはオフラインでも作れます。テレメトリはありません。
  - title: 表、JSON、CSV、TUI
    details: 人が読みやすい表、バージョン付きのスキーマを持つ JSON、フラットな CSV、バックグラウンドで同期する対話的なターミナル UI で出力できます。
---
