import { themes as prismThemes } from "prism-react-renderer";
import type { Config } from "@docusaurus/types";
import type * as Preset from "@docusaurus/preset-classic";
import tailwindPlugin from "./plugins/tailwind-config.cjs";

// This runs in Node.js - Don't use client-side code here (browser APIs, JSX...)

require("dotenv").config();

type SidebarItem = {
  type?: string;
  label?: string;
  items?: SidebarItem[];
  [key: string]: unknown;
};

const config: Config = {
  title: "goose | Your open source AI agent",
  tagline: "your local AI agent, automating engineering tasks seamlessly",
  favicon: "img/favicon.ico",

  // Set the production url of your site here
  url: "https://goose-docs.ai/",
  // Set the /<baseUrl>/ pathname under which your site is served
  // For GitHub pages deployment, it is often '/<projectName>/'
  baseUrl: process.env.TARGET_PATH || "/",

  // GitHub pages deployment config.
  // If you aren't using GitHub pages, you don't need these.
  organizationName: "aaif-goose", // Usually your GitHub org/user name.
  projectName: "goose", // Usually your repo name.

  onBrokenLinks: "throw",

  markdown: {
    hooks: {
      onBrokenMarkdownLinks: "warn",
    },
  },

  // Even if you don't use internationalization, you can use this field to set
  // useful metadata like html lang. For example, if your site is Chinese, you
  // may want to replace "en" with "zh-Hans".
  i18n: {
    defaultLocale: "en",
    locales: ["en"],
  },

  headTags: [
    {
      tagName: "link",
      attributes: {
        rel: "alternate",
        type: "text/plain",
        title: "LLM context",
        href: "/llms.txt",
      },
    },
  ],

  presets: [
    [
      "classic",
      {
        docs: {
          sidebarPath: "./sidebars.ts",
          async sidebarItemsGenerator(args) {
            const items = await args.defaultSidebarItemsGenerator(args);

            const contextEngineeringItems = [
              {
                type: "doc" as const,
                id: "mcp/memory-mcp",
                label: "Memory Extension",
              },
              {
                type: "doc" as const,
                id: "tutorials/rpi",
                label: "Research → Plan → Implement",
              },
            ];

            const addItemsToCategory = (
              sidebarItems: SidebarItem[],
              categoryLabel: string,
              extraItems: SidebarItem[],
            ) => {
              for (const item of sidebarItems) {
                if (item.type === "category" && item.label === categoryLabel) {
                  item.items.push(...extraItems);
                  return true;
                }

                if (
                  item.type === "category" &&
                  addItemsToCategory(item.items, categoryLabel, extraItems)
                ) {
                  return true;
                }
              }

              return false;
            };

            addItemsToCategory(items, "Context Engineering", contextEngineeringItems);

            return items;
          },
        },
        blog: {
          showReadingTime: true,
          readingTime: ({ content, frontMatter, defaultReadingTime }) =>
            frontMatter.reading_time ?? defaultReadingTime({ content }),
          feedOptions: {
            type: ["rss", "atom"],
            xslt: true,
          },
          // Useful options to enforce blogging best practices
          onInlineTags: "warn",
          onInlineAuthors: "warn",
          onUntruncatedBlogPosts: "warn",
          blogSidebarCount: "ALL",
          postsPerPage: 22,
        },
        theme: {
          customCss: [
            "./src/css/custom.css",
            "./src/css/extensions.css",
            "./src/css/tailwind.css",
          ],
        },
        gtag:
          process.env.NODE_ENV === "production"
            ? {
                trackingID: "G-ZS5D6SB4ZJ",
                anonymizeIP: true,
              }
            : false,
      } satisfies Preset.Options,
    ],
  ],
  plugins: [
    require.resolve("./plugins/custom-webpack.cjs"),
    [
      "@docusaurus/plugin-client-redirects",
      {
        redirects: [
          {
            from: "/docs/getting-started/using-goose-free",
            to: "/docs/getting-started/providers#using-goose-for-free",
          },
          {
            from: "/v1/docs/getting-started/providers",
            to: "/docs/getting-started/providers",
          },
          {
            from: "/v1/docs/getting-started/installation",
            to: "/docs/getting-started/installation",
          },
          {
            from: "/v1/docs/quickstart",
            to: "/docs/quickstart",
          },
          {
            from: "/v1/",
            to: "/",
          },
          {
            from: "/docs/guides/custom-extensions",
            to: "/docs/tutorials/custom-extensions",
          },
          {
            from: "/docs",
            to: "/docs/category/getting-started",
          },
          {
            from: "/v1/extensions",
            to: "/extensions",
          },
          {
            from: "/v1/extensions/detail/nondeveloper",
            to: "/docs/mcp/computer-controller-mcp",
          },
          {
            from: "/docs/guides/managing-goose-sessions",
            to: "/docs/guides/sessions/session-management",
          },
          {
            from: "/docs/guides/smart-context-management",
            to: "/docs/guides/sessions/smart-context-management",
          },
          {
            from: "/docs/guides/share-goose-sessions",
            to: "/docs/guides/recipes/session-recipes",
          },
          {
            from: "/docs/guides/session-recipes",
            to: "/docs/guides/recipes/session-recipes",
          },
          {
            from: "/docs/guides/recipe-reference",
            to: "/docs/guides/recipes/recipe-reference",
          },
          {
            from: "/docs/guides/recipes/sub-recipes",
            to: "/docs/guides/recipes/subrecipes",
          },
          {
            from: "/docs/tutorials/sub-recipes-in-parallel",
            to: "/docs/tutorials/subrecipes-in-parallel",
          },
          {
            from: "/docs/guides/tool-permissions",
            to: "/docs/guides/managing-tools/tool-permissions",
          },
          {
            from: "/docs/guides/adjust-tool-output",
            to: "/docs/guides/managing-tools/adjust-tool-output",
          },
          {
            from: "/docs/guides/goose-in-docker",
            to: "/docs/tutorials/goose-in-docker",
          },
          {
            from: "/docs/guides/multi-model/creating-plans",
            to: "/docs/guides/context-engineering",
          },
          {
            from: "/docs/guides/creating-plans",
            to: "/docs/guides/context-engineering",
          },
          {
            from: "/docs/guides/context-engineering/creating-plans",
            to: "/docs/guides/context-engineering",
          },
          {
            from: "/docs/tutorials/plan-feature-devcontainer-setup",
            to: "/docs/guides/context-engineering",
          },
          {
            from: "/docs/guides/config-file",
            to: "/docs/guides/config-files",
          },
          {
            from: "/docs/guides/using-persistent-instructions",
            to: "/docs/guides/context-engineering/using-persistent-instructions",
          },
          {
            from: "/docs/guides/subagents",
            to: "/docs/guides/context-engineering/subagents",
          },
          {
            from: "/docs/guides/prompt-templates",
            to: "/docs/guides/context-engineering/prompt-templates",
          },
          {
            from: "/docs/guides/goose-permissions",
            to: "/docs/guides/managing-tools/goose-permissions",
          },
          {
            from: "/docs/guides/using-goosehints",
            to: "/docs/guides/context-engineering/using-goosehints",
          },
          {
            from: "/docs/guides/managing-tools/hooks",
            to: "/docs/guides/context-engineering/hooks",
          },
          {
            from: "/docs/guides/managing-tools/plugins",
            to: "/docs/guides/context-engineering/plugins",
          },
          // MCP tutorial redirects - moved from /docs/tutorials/ to /docs/mcp/
          {
            from: "/docs/tutorials/agentql-mcp",
            to: "/docs/mcp/agentql-mcp",
          },
          {
            from: "/docs/tutorials/asana-mcp",
            to: "/docs/mcp/asana-mcp",
          },
          {
            from: "/docs/tutorials/blender-mcp",
            to: "/docs/mcp/blender-mcp",
          },
          {
            from: "/docs/tutorials/brave-mcp",
            to: "/docs/mcp/brave-mcp",
          },
          {
            from: "/docs/tutorials/browserbase-mcp",
            to: "/docs/mcp/browserbase-mcp",
          },
          {
            from: "/docs/tutorials/computer-controller-mcp",
            to: "/docs/mcp/computer-controller-mcp",
          },
          {
            from: "/docs/tutorials/context7-mcp",
            to: "/docs/mcp/context7-mcp",
          },
          {
            from: "/docs/tutorials/developer-mcp",
            to: "/docs/mcp/developer-mcp",
          },
          {
            from: "/docs/tutorials/elevenlabs-mcp",
            to: "/docs/mcp/elevenlabs-mcp",
          },
          {
            from: "/docs/tutorials/fetch-mcp",
            to: "/docs/mcp/fetch-mcp",
          },
          {
            from: "/docs/tutorials/figma-mcp",
            to: "/docs/mcp/figma-mcp",
          },
          {
            from: ["/docs/tutorials/filesystem-mcp", "/docs/mcp/filesystem-mcp"],
            to: "/docs/getting-started/using-extensions",
          },
          {
            from: "/docs/tutorials/github-mcp",
            to: "/docs/mcp/github-mcp",
          },
          {
            from: "/docs/tutorials/google-drive-mcp",
            to: "/docs/mcp/google-drive-mcp",
          },
          {
            from: "/docs/tutorials/google-maps-mcp",
            to: "/docs/mcp/google-maps-mcp",
          },
          {
            from: "/docs/tutorials/jetbrains-mcp",
            to: "/docs/mcp/jetbrains-mcp",
          },
          {
            from: "/docs/tutorials/knowledge-graph-mcp",
            to: "/docs/mcp/knowledge-graph-mcp",
          },
          {
            from: "/docs/tutorials/memory-mcp",
            to: "/docs/mcp/memory-mcp",
          },
          {
            from: "/docs/tutorials/nostrbook-mcp",
            to: "/docs/mcp/nostrbook-mcp",
          },
          {
            from: "/docs/tutorials/pdf-mcp",
            to: "/docs/mcp/pdf-mcp",
          },
          {
            from: "/docs/tutorials/pieces-mcp",
            to: "/docs/mcp/pieces-mcp",
          },
          {
            from: "/docs/tutorials/playwright-mcp",
            to: "/docs/mcp/playwright-mcp",
          },
          {
            from: "/docs/tutorials/postgres-mcp",
            to: "/docs/mcp/postgres-mcp",
          },
          {
            from: "/docs/tutorials/puppeteer-mcp",
            to: "/docs/mcp/puppeteer-mcp",
          },
          {
            from: "/docs/tutorials/reddit-mcp",
            to: "/docs/mcp/reddit-mcp",
          },
          {
            from: "/docs/tutorials/repomix-mcp",
            to: "/docs/mcp/repomix-mcp",
          },
          {
            from: "/docs/tutorials/selenium-mcp",
            to: "/docs/mcp/selenium-mcp",
          },
          {
            from: "/docs/tutorials/speech-mcp",
            to: "/docs/mcp/speech-mcp",
          },
          {
            from: "/docs/tutorials/square-mcp",
            to: "/docs/mcp/square-mcp",
          },
          {
            from: "/docs/tutorials/tavily-mcp",
            to: "/docs/mcp/tavily-mcp",
          },
          {
            from: "/docs/tutorials/tutorial-extension",
            to: "/docs/mcp/tutorial-mcp",
          },
          {
            from: "/docs/tutorials/vscode-mcp",
            to: "/docs/mcp/vs-code-mcp",
          },
          {
            from: "/docs/tutorials/youtube-transcript",
            to: "/docs/mcp/youtube-transcript-mcp",
          },
          {
            from: "/docs/guides/isolated-development-environments",
            to: "/docs/tutorials/isolated-development-environments",
          },
          {
            from: "/docs/experimental/subagents",
            to: "/docs/guides/context-engineering/subagents",
          },
          {
            from: "/docs/tutorials/lead-worker",
            to: "/docs/guides/context-engineering",
          },
        ],
      },
    ],
    tailwindPlugin,
    [
      require.resolve("./plugins/markdown-export.cjs"),
      {
        enabled: true,
      },
    ],
  ],
  themeConfig: {
    // Replace with your project's social card
    image: "img/home-banner.png",
    colorMode: {
      respectPrefersColorScheme: true,
    },
    navbar: {
      title: "",
      logo: {
        alt: "goose Logo", // TODO: replace logo assets with AAIF branding
        src: "img/logo_light.png",
        srcDark: "img/logo_dark.png",
      },
      items: [
        {
          to: "/docs/quickstart",
          label: "Quickstart",
          position: "left",
        },
        {
          to: "/docs/category/guides",
          position: "left",
          label: "Docs",
        },
        {
          to: "/docs/category/tutorials",
          position: "left",
          label: "Tutorials",
        },
        { to: "/blog", label: "Blog", position: "left" },
        {
          type: "dropdown",
          label: "Resources",
          position: "left",
          items: [
            {
              to: "/extensions",
              label: "Extensions",
            },
            {
              to: "/recipes",
              label: "Recipe Cookbook",
            },
            {
              to: "deeplink-generator",
              label: "Deeplink Generator",
            },
          ],
        },

        {
          href: "https://discord.gg/n8R5VaWDAn",
          label: "Discord",
          position: "right",
        },
        {
          href: "https://github.com/aaif-goose/goose",
          label: "GitHub",
          position: "right",
        },
      ],
    },
    footer: {
      links: [
        {
          title: "Quick Links",
          items: [
            {
              label: "Install goose",
              to: "docs/getting-started/installation",
            },
            {
              label: "Extensions",
              to: "/extensions",
            },
          ],
        },
        {
          title: "Community",
          items: [
            {
              label: "Spotlight",
              to: "community",
            },
            {
              label: "Discord",
              href: "https://discord.gg/n8R5VaWDAn",
            },
            {
              label: "YouTube",
              href: "https://www.youtube.com/@goose-oss",
            },
            {
              label: "LinkedIn",
              href: "https://www.linkedin.com/company/goose-oss",
            },
            {
              label: "Twitter / X",
              href: "https://x.com/goose_oss",
            },
            {
              label: "BlueSky",
              href: "https://bsky.app/profile/opensource.block.xyz",
            },
            {
              label: "Nostr",
              href: "https://njump.me/opensource@block.xyz",
            },
          ],
        },
        {
          title: "More",
          items: [
            {
              label: "Blog",
              to: "/blog",
            },
            {
              label: "GitHub",
              href: "https://github.com/aaif-goose/goose",
            },
          ],
        },
      ],
      copyright: `Copyright © ${new Date().getFullYear()} AAIF (Agentic AI Foundation)`,
    },
    prism: {
      theme: prismThemes.github,
      darkTheme: prismThemes.nightOwl,
    },
    announcementBar: {
      id: 'goose-aaif-announcement', // Increment on new announcements to reuse the bar
      content:
        '✨ goose has moved to the Agentic AI Foundation (AAIF): <a href="/blog/2026/04/07/goose-moves-to-aaif">Learn more</a>! ✨',
      backgroundColor: '#20232a',
      textColor: '#fff',
      isCloseable: true,
    },
  } satisfies Preset.ThemeConfig,
};

export default config;
