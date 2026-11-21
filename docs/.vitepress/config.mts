import { defineConfig } from 'vitepress'

export default defineConfig({
  title: 'ghusage',
  description: 'Know what you actually did on GitHub.',
  base: '/ghusage/',
  lang: 'en-US',
  cleanUrls: true,
  lastUpdated: false,
  ignoreDeadLinks: false,
  themeConfig: {
    nav: [
      { text: 'Guide', link: '/guide/getting-started' },
      { text: 'Reference', link: '/reference/cli' },
      { text: 'Metrics', link: '/reference/metrics' },
    ],
    sidebar: [
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
    ],
    search: {
      provider: 'local',
    },
    socialLinks: [
      { icon: 'github', link: 'https://github.com/tanahiro2010/ghusage' },
    ],
    outline: { level: [2, 3] },
  },
})
