import type { Config } from "@docusaurus/types";
import { themes as prismThemes } from "prism-react-renderer";

const DOCS_PATH =
  process.env.DOCS_PATH || (require("fs").existsSync("src") ? "src" : ".");

const config: Config = {
  title: "Docs Dev Preview",
  url: "http://localhost:3001",
  baseUrl: "/",
  trailingSlash: false,

  presets: [
    [
      "classic",
      {
        docs: {
          path: DOCS_PATH,
          routeBasePath: "/",
          sidebarPath: "./sidebars.ts",
          remarkPlugins: [require("remark-math")],
          rehypePlugins: [require("rehype-katex")],
          versions: {
            current: {
              label: `unstable`,
            },
          },
        },
        blog: false,
        pages: false,
        theme: {
          customCss: "./styles.css",
        },
      },
    ],
  ],

  plugins: [
    [
      "@cmfcmf/docusaurus-search-local",
      {
        indexDocs: true,
        indexBlog: false,
        indexPages: false,
        language: "en",
        style: undefined,
      },
    ],
  ],

  themeConfig: {
    navbar: {
      title: "Note Transport Layer",
    },
    prism: {
      theme: prismThemes.github,
      darkTheme: prismThemes.dracula,
      additionalLanguages: ["protobuf", "toml", "bash", "rust"],
    },
  },
};

export default config;
