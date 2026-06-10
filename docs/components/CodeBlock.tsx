import { highlight } from '@/lib/highlight';
import type { Lang } from '@/lib/types';
import { CopyButton } from './CopyButton';

export function CodeBlock({ code, lang, title }: { code: string; lang: Lang; title?: string }) {
  const tokens = highlight(code, lang);
  return (
    <figure className="codeblock">
      <figcaption className="codeblock-bar">
        <span className="codeblock-dot" aria-hidden />
        <span className="codeblock-title">{title ?? lang}</span>
        <span className="codeblock-lang">{lang}</span>
        <CopyButton text={code} />
      </figcaption>
      <pre>
        <code>
          {tokens.map((t, i) =>
            t.cls ? (
              <span key={i} className={`tok-${t.cls}`}>
                {t.text}
              </span>
            ) : (
              t.text
            ),
          )}
        </code>
      </pre>
    </figure>
  );
}
