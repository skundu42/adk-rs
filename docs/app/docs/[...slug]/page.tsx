import Link from 'next/link';
import { notFound } from 'next/navigation';
import type { Metadata } from 'next';
import { Blocks } from '@/components/Blocks';
import { Toc } from '@/components/Toc';
import { getPage, groupOf, orderedSlugs, prevNext, REPO_URL } from '@/lib/registry';

export function generateStaticParams() {
  return orderedSlugs.map((slug) => ({ slug: slug.split('/') }));
}

export const dynamicParams = false;

interface Props {
  params: Promise<{ slug: string[] }>;
}

export async function generateMetadata({ params }: Props): Promise<Metadata> {
  const { slug } = await params;
  const page = getPage(slug.join('/'));
  if (!page) return {};
  return { title: page.title, description: page.description };
}

export default async function DocPage({ params }: Props) {
  const { slug } = await params;
  const page = getPage(slug.join('/'));
  if (!page) notFound();

  const group = groupOf(page.slug);
  const { prev, next } = prevNext(page.slug);

  return (
    <>
      <main className="doc-main">
        <article className="doc">
          <div className="doc-eyebrow">
            <span>{group?.label ?? 'Docs'}</span>
            {page.srcPath ? (
              <a
                className="src-link"
                href={`${REPO_URL}/blob/main/${page.srcPath}`}
                target="_blank"
                rel="noreferrer"
              >
                {page.srcPath} ↗
              </a>
            ) : null}
          </div>
          <h1>{page.title}</h1>
          <p className="doc-desc">{page.description}</p>
          <Blocks blocks={page.blocks} />
          <nav className="pager" aria-label="Pagination">
            {prev ? (
              <Link href={`/docs/${prev.slug}`}>
                <span className="pager-dir">← Previous</span>
                <span className="pager-title">{prev.title}</span>
              </Link>
            ) : (
              <span />
            )}
            {next ? (
              <Link href={`/docs/${next.slug}`} className="pager-next">
                <span className="pager-dir">Next →</span>
                <span className="pager-title">{next.title}</span>
              </Link>
            ) : null}
          </nav>
        </article>
      </main>
      <div className="toc-rail">
        <Toc blocks={page.blocks} />
      </div>
    </>
  );
}
