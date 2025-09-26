// @ts-check
// `@type` JSDoc annotations allow editor autocompletion and type checking
// (when paired with `@ts-check`).
// There are various equivalent ways to declare your Docusaurus config.
// See: https://docusaurus.io/docs/api/docusaurus-config

import { themes as prismThemes } from "prism-react-renderer"
import path from "path"
import fs from "fs"

// This runs in Node.js - Don't use client-side code here (browser APIs, JSX...)
function getVersions() {
  const versionsPath = path.resolve(__dirname, "versions.json")
  if (fs.existsSync(versionsPath)) {
    return require(versionsPath);
  }
  return ["current"];
}
function getDocsVersions() {
  let currentVersion = getLatestVersion()
  let versions = getVersions()
  const result = {}

  versions.forEach((version) => {
    if (version === "current") {
      result[version] = {
        label: "dev",
        banner: "unreleased",
        badge: false,
      }
    } else if (version !== currentVersion) {
      result[version] = {
        banner: "unmaintained",
        badge: false,
      }
    } else {
      result[version] = {
        banner: "none",
        badge: false,
      }
    }
  })
  return result
}

function getLatestVersion() {
  let versions = getVersions()
  if (versions.length < 1) {
    return "current"
  }
  return versions[0]
}

/** @type {import('@docusaurus/types').Config} */
const config = {
  title: "Trident",
  tagline: "Azure Linux deployment and update agent",
  // favicon: 'img/favicon.ico',

  // Future flags, see https://docusaurus.io/docs/api/docusaurus-config#future
  future: {
    v4: true, // Improve compatibility with the upcoming Docusaurus v4
  },

  // When GitHub Pages is public
  // url: 'https://microsoft.github.io',
  // baseUrl: '/trident/',

  // While GitHub Pages is private
  url: "https://vigilant-adventure-5jnm363.pages.github.io/",
  baseUrl: "/",

  // GitHub pages deployment config.
  // If you aren't using GitHub pages, you don't need these.
  organizationName: "Microsoft", // Usually your GitHub org/user name.
  projectName: "trident", // Usually your repo name.

  onBrokenLinks: "throw",

  markdown: {
    hooks: {
      onBrokenMarkdownLinks: "throw",
    },
  },

  // Even if you don't use internationalization, you can use this field to set
  // useful metadata like html lang. For example, if your site is Chinese, you
  // may want to replace "en" with "zh-Hans".
  i18n: {
    defaultLocale: "en",
    locales: ["en"],
  },

  presets: [
    [
      "classic",
      /** @type {import('@docusaurus/preset-classic').Options} */
      ({
        docs: {
          routeBasePath: "/docs/",
          lastVersion: getLatestVersion(),
          versions: getDocsVersions(),
          sidebarPath: "./sidebars.js",
          // Please change this to your repo.
          // Remove this to remove the "edit this page" links.
          editUrl:
            "https://github.com/microsoft/trident/tree/main/docs/",
        },
        theme: {
          customCss: "./src/css/custom.css",
        },
      }),
    ],
  ],

  themeConfig:
    /** @type {import('@docusaurus/preset-classic').ThemeConfig} */
    ({
      // image: 'img/trident-social-card.jpg',
      navbar: {
        title: "Trident",
        // logo: {
        //   alt: 'Trident Logo',
        //   src: 'img/logo.svg',
        // },
        items: [
          {
            type: "doc",
            docId: "Trident",
            position: "left",
            label: "Docs",
          },
          {
            type: "docsVersionDropdown",
            position: "left",
          },
          {
            href: "https://github.com/microsoft/trident",
            label: "GitHub",
            position: "right",
          },
        ],
      },
      footer: {
        style: "dark",
        links: [
          {
            title: "Docs",
            items: [
              {
                label: "Docs",
                to: "/docs/Trident",
              },
            ],
          },
          {
            title: "Community",
            items: [
              // {
              //   label: 'Stack Overflow',
              //   href: 'https://stackoverflow.com/questions/tagged/trident',
              // },
              // {
              //   label: 'Discord',
              //   href: 'https://discordapp.com/invite/trident',
              // },
              // {
              //   label: 'X',
              //   href: 'https://x.com/trident',
              // },
            ],
          },
          {
            title: "More",
            items: [
              {
                label: "GitHub",
                href: "https://github.com/microsoft/trident",
              },
            ],
          },
        ],
        // copyright: `Copyright © ${new Date().getFullYear()} Microsoft.`,
      },
      prism: {
        theme: prismThemes.github,
        darkTheme: prismThemes.dracula,
        additionalLanguages: ["bash"],
      },
    }),
  themes: [
    [
      require.resolve("@easyops-cn/docusaurus-search-local"),
      /** @type {import("@easyops-cn/docusaurus-search-local").PluginOptions} */
      {
        hashed: true,
      },
    ],
    "@docusaurus/theme-mermaid",
  ],
  markdown: {
    mermaid: true,
  },
}
export default config;
