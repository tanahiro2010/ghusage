import { defineConfig, type DefaultTheme } from 'vitepress'

const enNav: DefaultTheme.NavItem[] = [
  { text: 'Guide', link: '/guide/getting-started' },
  { text: 'Reference', link: '/reference/cli' },
  { text: 'Metrics', link: '/reference/metrics' },
]

const enSidebar: DefaultTheme.Sidebar = [
  {
    text: 'Guide',
    items: [
      { text: 'Getting started', link: '/guide/getting-started' },
      { text: 'Authentication', link: '/guide/authentication' },
      { text: 'Syncing and the cache', link: '/guide/syncing' },
      { text: 'Terminal UI', link: '/guide/tui' },
    ],
  },
  {
    text: 'Reference',
    items: [
      { text: 'CLI', link: '/reference/cli' },
      { text: 'Metric definitions', link: '/reference/metrics' },
      { text: 'Periods and time zones', link: '/reference/time' },
      { text: 'JSON and CSV output', link: '/reference/json' },
      { text: 'Known limitations', link: '/reference/limitations' },
      { text: 'Privacy', link: '/reference/privacy' },
    ],
  },
]

const jaNav: DefaultTheme.NavItem[] = [
  { text: 'ガイド', link: '/ja/guide/getting-started' },
  { text: 'リファレンス', link: '/ja/reference/cli' },
  { text: '指標', link: '/ja/reference/metrics' },
]

const jaSidebar: DefaultTheme.Sidebar = [
  {
    text: 'ガイド',
    items: [
      { text: 'はじめに', link: '/ja/guide/getting-started' },
      { text: '認証', link: '/ja/guide/authentication' },
      { text: '同期とキャッシュ', link: '/ja/guide/syncing' },
      { text: 'ターミナル UI', link: '/ja/guide/tui' },
    ],
  },
  {
    text: 'リファレンス',
    items: [
      { text: 'CLI', link: '/ja/reference/cli' },
      { text: '指標の定義', link: '/ja/reference/metrics' },
      { text: '期間とタイムゾーン', link: '/ja/reference/time' },
      { text: 'JSON と CSV の出力', link: '/ja/reference/json' },
      { text: '既知の制約', link: '/ja/reference/limitations' },
      { text: 'プライバシー', link: '/ja/reference/privacy' },
    ],
  },
]

export default defineConfig({
  title: 'ghusage',
  base: '/',
  cleanUrls: true,
  lastUpdated: false,
  ignoreDeadLinks: false,
  locales: {
    root: {
      label: 'English',
      lang: 'en-US',
      description: 'Know what you actually did on GitHub.',
      themeConfig: {
        nav: enNav,
        sidebar: enSidebar,
      },
    },
    ja: {
      label: '日本語',
      lang: 'ja-JP',
      link: '/ja/',
      description: 'GitHub で実際に何をしたのかを把握する。',
      themeConfig: {
        nav: jaNav,
        sidebar: jaSidebar,
        outline: { level: [2, 3], label: 'このページの内容' },
        docFooter: { prev: '前のページ', next: '次のページ' },
        lastUpdated: { text: '最終更新' },
        darkModeSwitchLabel: '外観',
        lightModeSwitchTitle: 'ライトモードに切り替える',
        darkModeSwitchTitle: 'ダークモードに切り替える',
        sidebarMenuLabel: 'メニュー',
        returnToTopLabel: 'ページの先頭へ',
        langMenuLabel: '言語を切り替える',
        skipToContentLabel: '本文へ移動',
        notFound: {
          title: 'ページが見つかりません',
          quote: 'お探しのページは移動または削除された可能性があります。',
          linkLabel: 'ホームへ戻る',
          linkText: 'ホームへ戻る',
        },
      },
    },
  },
  themeConfig: {
    search: {
      provider: 'local',
      options: {
        locales: {
          ja: {
            translations: {
              button: {
                buttonText: '検索',
                buttonAriaLabel: '検索',
              },
              modal: {
                displayDetails: '詳細を表示',
                resetButtonTitle: '検索をリセット',
                backButtonTitle: '検索を閉じる',
                noResultsText: '該当する結果はありません',
                footer: {
                  selectText: '選択',
                  selectKeyAriaLabel: 'Enter キー',
                  navigateText: '移動',
                  navigateUpKeyAriaLabel: '上矢印キー',
                  navigateDownKeyAriaLabel: '下矢印キー',
                  closeText: '閉じる',
                  closeKeyAriaLabel: 'Esc キー',
                },
              },
            },
          },
        },
      },
    },
    socialLinks: [
      { icon: 'github', link: 'https://github.com/tanahiro2010/ghusage' },
    ],
    outline: { level: [2, 3] },
  },
})
