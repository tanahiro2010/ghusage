import { existsSync } from 'node:fs'
import { join } from 'node:path'
import { defineConfig, type DefaultTheme, type HeadConfig } from 'vitepress'

const hostname = 'https://ghusage.under18.dev'
const siteName = 'ghusage'
const ogImage = `${hostname}/overview.webp`

const enDescription =
  'ghusage is a CLI and terminal UI that analyzes your GitHub activity: commits, pull requests, reviews, comments, issues, and everything that happens inside pull requests.'
const jaDescription =
  'ghusage は GitHub での活動を分析する CLI とターミナル UI です。コミット、プルリクエスト、レビュー、コメント、Issue、プルリクエストの中で起きたことまで集計します。'

// Maps a source path such as `ja/guide/tui.md` to its public URL path.
// The site uses clean URLs, so `.md` is dropped and `index` maps to the directory.
function toUrlPath(relativePath: string): string {
  const path = relativePath.replace(/\.md$/, '').replace(/(^|\/)index$/, '$1')
  return `/${path}`
}

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
      { text: 'Profile README', link: '/guide/profile-readme' },
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
      { text: 'プロフィール README', link: '/ja/guide/profile-readme' },
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
  // Also provides `lastmod` for the sitemap; the docs workflow fetches full history.
  lastUpdated: true,
  ignoreDeadLinks: false,
  description: enDescription,
  head: [
    ['meta', { property: 'og:site_name', content: siteName }],
    ['meta', { property: 'og:type', content: 'website' }],
    ['meta', { property: 'og:image', content: ogImage }],
    ['meta', { property: 'og:image:alt', content: 'ghusage overview output in a terminal' }],
    ['meta', { name: 'twitter:card', content: 'summary_large_image' }],
    ['meta', { name: 'twitter:image', content: ogImage }],
  ],
  sitemap: {
    hostname,
  },
  transformPageData(pageData, { siteConfig }) {
    if (pageData.isNotFound) return

    const isJa = pageData.relativePath.startsWith('ja/')
    const enRelative = isJa ? pageData.relativePath.slice('ja/'.length) : pageData.relativePath
    const jaRelative = `ja/${enRelative}`
    const hasEn = existsSync(join(siteConfig.srcDir, enRelative))
    const hasJa = existsSync(join(siteConfig.srcDir, jaRelative))

    const url = `${hostname}${toUrlPath(pageData.relativePath)}`
    const title = pageData.title ? `${pageData.title} | ${siteName}` : siteName
    const description =
      pageData.frontmatter.description || pageData.description || (isJa ? jaDescription : enDescription)

    const head: HeadConfig[] = [
      ['link', { rel: 'canonical', href: url }],
      ['meta', { property: 'og:title', content: title }],
      ['meta', { property: 'og:description', content: description }],
      ['meta', { property: 'og:url', content: url }],
      ['meta', { property: 'og:locale', content: isJa ? 'ja_JP' : 'en_US' }],
      ['meta', { name: 'twitter:title', content: title }],
      ['meta', { name: 'twitter:description', content: description }],
    ]

    if (hasEn && hasJa) {
      head.push(['meta', { property: 'og:locale:alternate', content: isJa ? 'en_US' : 'ja_JP' }])
      const enUrl = `${hostname}${toUrlPath(enRelative)}`
      const jaUrl = `${hostname}${toUrlPath(jaRelative)}`
      head.push(
        ['link', { rel: 'alternate', hreflang: 'en', href: enUrl }],
        ['link', { rel: 'alternate', hreflang: 'ja', href: jaUrl }],
        ['link', { rel: 'alternate', hreflang: 'x-default', href: enUrl }],
      )
    }

    pageData.frontmatter.head ??= []
    pageData.frontmatter.head.push(...head)
  },
  locales: {
    root: {
      label: 'English',
      lang: 'en-US',
      description: enDescription,
      themeConfig: {
        nav: enNav,
        sidebar: enSidebar,
      },
    },
    ja: {
      label: '日本語',
      lang: 'ja-JP',
      link: '/ja/',
      description: jaDescription,
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
