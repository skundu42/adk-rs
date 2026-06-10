/**
 * Tiny dependency-free syntax highlighter.
 *
 * Each language is a list of (regex, class) rules tried in order at the
 * current position; first match wins. Unmatched characters pass through as
 * plain text. Output is a token list rendered to <span>s by CodeBlock —
 * no innerHTML anywhere.
 */

import type { Lang } from './types';

export interface Token {
  text: string;
  /** css class suffix, e.g. 'k' renders as .tok-k; undefined = plain */
  cls?: string;
}

type Rule = [RegExp, string];

const RUST_KEYWORDS =
  'as|async|await|break|const|continue|crate|dyn|else|enum|extern|false|fn|for|if|impl|in|let|loop|match|mod|move|mut|pub|ref|return|self|Self|static|struct|super|trait|true|type|unsafe|use|where|while';

const rules: Record<Lang, Rule[]> = {
  rust: [
    [/^\/\/[^\n]*/, 'c'],
    [/^\/\*[\s\S]*?\*\//, 'c'],
    [/^#!?\[[^\]]*\]/, 'a'],
    [/^"(?:\\.|[^"\\])*"/, 's'],
    [/^'(?:\\.|[^'\\])'/, 's'],
    [/^'[a-zA-Z_][a-zA-Z0-9_]*\b(?!')/, 'l'],
    [/^\b[a-z_][a-zA-Z0-9_]*!/, 'm'],
    [new RegExp(`^\\b(?:${RUST_KEYWORDS})\\b`), 'k'],
    [/^\b[A-Z][A-Za-z0-9_]*\b/, 't'],
    [/^\b0x[0-9a-fA-F_]+\b|^\b\d[\d_]*(?:\.\d[\d_]*)?(?:[uif](?:8|16|32|64|size)?)?\b/, 'n'],
    [/^\b[a-z_][a-zA-Z0-9_]*(?=\()/, 'f'],
  ],
  toml: [
    [/^#[^\n]*/, 'c'],
    [/^"(?:\\.|[^"\\])*"/, 's'],
    [/^\[[^\]\n]*\]/, 'a'],
    [/^\b(?:true|false)\b/, 'k'],
    [/^\b\d[\d._]*\b/, 'n'],
    [/^[A-Za-z0-9_-]+(?=\s*=)/, 'f'],
  ],
  bash: [
    [/^#[^\n]*/, 'c'],
    [/^"(?:\\.|[^"\\])*"/, 's'],
    [/^'[^']*'/, 's'],
    [/^\$\{[^}]+\}|^\$[A-Za-z_][A-Za-z0-9_]*/, 'v'],
    [/^(?:^|(?<=\s))--?[A-Za-z][\w-]*/, 'a'],
    [/^\b(?:cargo|npm|cd|export|curl|docker|git|echo|run|build|test|install|add)\b/, 'k'],
  ],
  json: [
    [/^"(?:\\.|[^"\\])*"(?=\s*:)/, 'f'],
    [/^"(?:\\.|[^"\\])*"/, 's'],
    [/^\b(?:true|false|null)\b/, 'k'],
    [/^-?\b\d+(?:\.\d+)?(?:[eE][+-]?\d+)?\b/, 'n'],
  ],
  text: [],
};

export function highlight(code: string, lang: Lang): Token[] {
  const ruleset = rules[lang] ?? [];
  const tokens: Token[] = [];
  let rest = code;
  let plain = '';

  const flush = () => {
    if (plain) {
      tokens.push({ text: plain });
      plain = '';
    }
  };

  outer: while (rest.length > 0) {
    for (const [re, cls] of ruleset) {
      const m = re.exec(rest);
      if (m && m[0].length > 0) {
        flush();
        tokens.push({ text: m[0], cls });
        rest = rest.slice(m[0].length);
        continue outer;
      }
    }
    plain += rest[0];
    rest = rest.slice(1);
  }
  flush();
  return tokens;
}
