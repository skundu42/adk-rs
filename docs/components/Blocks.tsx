import type { Block } from '@/lib/types';
import { CodeBlock } from './CodeBlock';
import { Inline, anchorId } from './inline';

export function Blocks({ blocks }: { blocks: Block[] }) {
  return (
    <>
      {blocks.map((b, i) => (
        <BlockView key={i} block={b} />
      ))}
    </>
  );
}

function BlockView({ block }: { block: Block }) {
  switch (block.kind) {
    case 'p':
      return (
        <p>
          <Inline text={block.text} />
        </p>
      );
    case 'lede':
      return (
        <p className="lede">
          <Inline text={block.text} />
        </p>
      );
    case 'h2': {
      const id = anchorId(block.text);
      return (
        <h2 id={id}>
          <a className="anchor" href={`#${id}`} aria-hidden tabIndex={-1}>
            §
          </a>
          <Inline text={block.text} />
        </h2>
      );
    }
    case 'h3': {
      const id = anchorId(block.text);
      return (
        <h3 id={id}>
          <Inline text={block.text} />
        </h3>
      );
    }
    case 'code':
      return <CodeBlock code={block.code} lang={block.lang} title={block.title} />;
    case 'list':
      return block.ordered ? (
        <ol>
          {block.items.map((it, i) => (
            <li key={i}>
              <Inline text={it} />
            </li>
          ))}
        </ol>
      ) : (
        <ul>
          {block.items.map((it, i) => (
            <li key={i}>
              <Inline text={it} />
            </li>
          ))}
        </ul>
      );
    case 'table':
      return (
        <div className="table-wrap">
          <table>
            <thead>
              <tr>
                {block.head.map((h, i) => (
                  <th key={i}>
                    <Inline text={h} />
                  </th>
                ))}
              </tr>
            </thead>
            <tbody>
              {block.rows.map((row, i) => (
                <tr key={i}>
                  {row.map((cell, j) => (
                    <td key={j}>
                      <Inline text={cell} />
                    </td>
                  ))}
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      );
    case 'callout':
      return (
        <aside className={`callout callout-${block.tone}`}>
          <span className="callout-label">
            {block.title ?? (block.tone === 'warn' ? 'Warning' : block.tone === 'tip' ? 'Tip' : 'Note')}
          </span>
          <p>
            <Inline text={block.text} />
          </p>
        </aside>
      );
    case 'api':
      return (
        <dl className="api-list">
          {block.entries.map((e, i) => (
            <div className="api-row" key={i}>
              <dt>
                <code>{e.sig}</code>
              </dt>
              <dd>
                <Inline text={e.desc} />
              </dd>
            </div>
          ))}
        </dl>
      );
    case 'hr':
      return <hr />;
  }
}
