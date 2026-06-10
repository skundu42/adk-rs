import { Sidebar } from '@/components/Sidebar';
import { nav, getPage } from '@/lib/registry';

export default function DocsLayout({ children }: { children: React.ReactNode }) {
  const titles: Record<string, string> = {};
  for (const group of nav) {
    for (const slug of group.slugs) {
      titles[slug] = getPage(slug)?.title ?? slug;
    }
  }
  return (
    <div className="docs-shell">
      <Sidebar nav={nav} titles={titles} />
      {children}
    </div>
  );
}
