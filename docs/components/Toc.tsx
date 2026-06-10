import type { Block } from '@/lib/types';
import { anchorId } from './inline';

export function Toc({ blocks }: { blocks: Block[] }) {
  const headings = blocks.filter((b): b is Extract<Block, { kind: 'h2' }> => b.kind === 'h2');
  if (headings.length < 2) return null;
  return (
    <nav className="toc" aria-label="On this page">
      <span className="toc-label">On this page</span>
      <ul>
        {headings.map((h) => (
          <li key={h.text}>
            <a href={`#${anchorId(h.text)}`}>{h.text.replace(/`/g, '')}</a>
          </li>
        ))}
      </ul>
    </nav>
  );
}
