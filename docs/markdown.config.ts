import {
  transformerNotationDiff,
  transformerMetaHighlight,
  transformerNotationFocus,
  transformerMetaWordHighlight,
  transformerNotationHighlight,
  transformerNotationErrorLevel,
  transformerNotationWordHighlight
} from "@shikijs/transformers"
import remarkToc from "remark-toc"
import rehypeSlug from "rehype-slug"
import { visit } from "unist-util-visit"
import remarkMathPlugin from "remark-math"
import rehypeKatexPlugin from "rehype-katex"
import rehypeMathjaxPlugin from "rehype-mathjax"
import remarkSmartypants from "remark-smartypants"
import type { AstroUserConfig } from "astro/config"
import { escapeHTML } from "astro/runtime/server/escape.js"
import rehypeAutolinkHeadings from "rehype-autolink-headings"
// import { transformerCopyButton } from "@rehype-pretty/transformers"
import { rendererRich, transformerTwoslash } from "@shikijs/twoslash"
import { rehypeHeadingIds, type RemarkPlugin, type ShikiConfig } from "@astrojs/markdown-remark"

type Markdown = AstroUserConfig["markdown"]

export const shikiConfig = {
  theme: "houston",
  transformers: [
    transformerTwoslash({
      explicitTrigger: /\btwoslash\b/,
      renderer: rendererRich({ jsdoc: true })
    }),
    transformerNotationDiff(),
    transformerMetaHighlight(),
    transformerNotationFocus(),
    transformerMetaWordHighlight(),
    transformerNotationHighlight(),
    transformerNotationErrorLevel(),
    transformerNotationWordHighlight()
    // transformerCopyButton({ visibility: "hover", feedbackDuration: 3_000 })
  ]
} satisfies ShikiConfig

export const markdownConfiguration = {
  gfm: true,
  shikiConfig,
  smartypants: false,
  syntaxHighlight: "shiki",
  remarkRehype: {
    allowDangerousHtml: true,
    clobberPrefix: "union-docs-",
    passThrough: ["code", "root"]
  },
  remarkPlugins: [
    mermaid(),
    remarkMathPlugin,
    remarkSmartypants as RemarkPlugin,
    [remarkToc, { heading: "contents", prefix: "toc-" }]
  ],
  rehypePlugins: [
    rehypeSlug,
    rehypeHeadingIds,
    rehypeKatexPlugin,
    rehypeMathjaxPlugin,
    [rehypeAutolinkHeadings, { behavior: "wrap" }]
  ]
} satisfies Markdown

export function mermaid(): RemarkPlugin<Array<any>> {
  return () => tree => {
    visit(tree, "code", node => {
      if (node.lang !== "mermaid") return
      // @ts-expect-error
      node.type = "html"
      node.value = /* html */ `<div class="mermaid">${escapeHTML(node.value)}</div>`
    })
  }
}
