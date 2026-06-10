import React from 'react';
import Link from 'next/link';

/**
 * Renders the inline markdown subset used in content files:
 * `code`, **bold**, *italic*, [label](href). Internal hrefs (starting
 * with "/") use next/link.
 */
export function Inline({ text }: { text: string }) {
  return <>{parse(text)}</>;
}

const PATTERN = /(`[^`]+`)|(\*\*[^*]+\*\*)|(\*[^*]+\*)|(\[[^\]]+\]\([^)]+\))/g;

function parse(text: string): React.ReactNode[] {
  const nodes: React.ReactNode[] = [];
  let last = 0;
  let key = 0;
  for (const m of text.matchAll(PATTERN)) {
    const idx = m.index ?? 0;
    if (idx > last) nodes.push(text.slice(last, idx));
    const tok = m[0];
    if (tok.startsWith('`')) {
      nodes.push(<code key={key++}>{tok.slice(1, -1)}</code>);
    } else if (tok.startsWith('**')) {
      nodes.push(<strong key={key++}>{parse(tok.slice(2, -2))}</strong>);
    } else if (tok.startsWith('*')) {
      nodes.push(<em key={key++}>{parse(tok.slice(1, -1))}</em>);
    } else {
      const inner = /\[([^\]]+)\]\(([^)]+)\)/.exec(tok);
      if (inner) {
        const [, label, href] = inner;
        nodes.push(
          href.startsWith('/') ? (
            <Link key={key++} href={href}>
              {parse(label)}
            </Link>
          ) : (
            <a key={key++} href={href} target="_blank" rel="noreferrer">
              {parse(label)}
            </a>
          ),
        );
      }
    }
    last = idx + tok.length;
  }
  if (last < text.length) nodes.push(text.slice(last));
  return nodes;
}

/** Slug for heading anchors: "The `Runner` builder" -> "the-runner-builder" */
export function anchorId(text: string): string {
  return text
    .toLowerCase()
    .replace(/`/g, '')
    .replace(/[^a-z0-9\s-]/g, '')
    .trim()
    .replace(/\s+/g, '-');
}
