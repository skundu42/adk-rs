/**
 * Content model for documentation pages.
 *
 * Every docs page is a plain data file in `content/` exporting a `DocPage`.
 * Inline text in `text`, `items`, table cells and `desc` fields supports a
 * small markdown subset: `code`, **bold**, *italic*, and [label](href).
 */

export type Lang = 'rust' | 'toml' | 'bash' | 'json' | 'text';

export type Block =
  /** Body paragraph. */
  | { kind: 'p'; text: string }
  /** Large italic lede paragraph, used right after the title. */
  | { kind: 'lede'; text: string }
  /** Section heading — collected into the on-page table of contents. */
  | { kind: 'h2'; text: string }
  | { kind: 'h3'; text: string }
  /** Highlighted code panel. `title` renders in the panel header. */
  | { kind: 'code'; lang: Lang; title?: string; code: string }
  | { kind: 'list'; ordered?: boolean; items: string[] }
  | { kind: 'table'; head: string[]; rows: string[][] }
  | { kind: 'callout'; tone: 'note' | 'warn' | 'tip'; title?: string; text: string }
  /** API reference rows: a monospace signature plus a description. */
  | { kind: 'api'; entries: { sig: string; desc: string }[] }
  | { kind: 'hr' };

export interface DocPage {
  /** URL path under /docs/, e.g. "llm-agent" or "examples/weather-agent". */
  slug: string;
  title: string;
  /** One-sentence summary shown under the title and in <meta>. */
  description: string;
  /** Path in the repo this page documents, e.g. "src/agents/llm_agent.rs". */
  srcPath?: string;
  blocks: Block[];
}

export interface NavGroup {
  label: string;
  slugs: string[];
}
