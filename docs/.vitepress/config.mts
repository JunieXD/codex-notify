import { defineConfig } from 'vitepress'

export default defineConfig({
  lang: 'zh-CN',
  title: 'codex-notify',
  titleTemplate: ':title | codex-notify',
  description: 'Codex 任务完成或中断时，通过飞书及时提醒你的跨平台命令行工具。',
  base: '/codex-notify/',
  cleanUrls: true,
  lastUpdated: true,
  sitemap: {
    hostname: 'https://juniexd.github.io/codex-notify/'
  },
  vite: {
    publicDir: '../assets'
  },
  head: [
    ['link', { rel: 'icon', type: 'image/svg+xml', href: '/codex-notify/logo.svg' }],
    ['meta', { name: 'theme-color', content: '#5b6ee1' }],
    ['meta', { property: 'og:type', content: 'website' }],
    ['meta', { property: 'og:title', content: 'codex-notify' }],
    [
      'meta',
      {
        property: 'og:description',
        content: '让 Codex 完成任务或意外中断时，及时在飞书提醒你。'
      }
    ],
    [
      'meta',
      {
        property: 'og:image',
        content: 'https://juniexd.github.io/codex-notify/logo.svg'
      }
    ]
  ],
  themeConfig: {
    logo: '/logo.svg',
    siteTitle: 'codex-notify',
    nav: [
      { text: '首页', link: '/' },
      { text: '使用指南', link: '/guide/getting-started' },
      { text: '命令手册', link: '/guide/commands' },
      { text: '工作原理', link: '/concepts/how-it-works' },
      {
        text: '更新公告',
        link: 'https://github.com/JunieXD/codex-notify/releases'
      }
    ],
    sidebar: [
      {
        text: '开始使用',
        items: [
          { text: '快速开始', link: '/guide/getting-started' },
          { text: '配置飞书应用', link: '/guide/feishu-setup' }
        ]
      },
      {
        text: '使用手册',
        items: [
          { text: '命令手册', link: '/guide/commands' },
          { text: '通知内容与时机', link: '/guide/notifications' },
          { text: 'Codex 配置与共存', link: '/guide/configuration' },
          { text: '升级与卸载', link: '/guide/update-uninstall' },
          { text: '排查常见问题', link: '/guide/troubleshooting' }
        ]
      },
      {
        text: '原理与安全',
        items: [
          { text: '工作原理', link: '/concepts/how-it-works' },
          { text: '隐私与安全', link: '/reference/security' },
          { text: '支持平台', link: '/reference/platforms' }
        ]
      },
      {
        text: '维护者文档',
        collapsed: true,
        items: [
          { text: '项目规格', link: '/specification' },
          { text: '发布流程', link: '/releasing' }
        ]
      }
    ],
    socialLinks: [
      { icon: 'github', link: 'https://github.com/JunieXD/codex-notify' }
    ],
    search: {
      provider: 'local',
      options: {
        translations: {
          button: {
            buttonText: '搜索文档',
            buttonAriaLabel: '搜索文档'
          },
          modal: {
            noResultsText: '没有找到相关内容',
            resetButtonTitle: '清除搜索',
            footer: {
              selectText: '选择',
              navigateText: '切换',
              closeText: '关闭'
            }
          }
        }
      }
    },
    outline: {
      level: [2, 3],
      label: '本页内容'
    },
    editLink: {
      pattern: 'https://github.com/JunieXD/codex-notify/edit/main/docs/:path',
      text: '在 GitHub 上编辑此页'
    },
    lastUpdated: {
      text: '最后更新于',
      formatOptions: {
        dateStyle: 'medium',
        timeStyle: 'short'
      }
    },
    docFooter: {
      prev: '上一篇',
      next: '下一篇'
    },
    darkModeSwitchLabel: '外观',
    lightModeSwitchTitle: '切换到浅色模式',
    darkModeSwitchTitle: '切换到深色模式',
    sidebarMenuLabel: '文档目录',
    returnToTopLabel: '返回顶部',
    langMenuLabel: '切换语言',
    externalLinkIcon: true,
    notFound: {
      title: '页面没有找到',
      quote: '这个链接可能已经移动，请从文档首页重新查找。',
      linkLabel: '返回首页',
      linkText: '返回首页'
    },
    footer: {
      message: '基于 MIT 许可证发布',
      copyright: 'Copyright © 2026 JunieXD'
    }
  }
})
