'use client';

import { useState } from 'react';
import Link from 'next/link';
import { usePathname } from 'next/navigation';
import type { NavGroup } from '@/lib/types';

export function Sidebar({
  nav,
  titles,
}: {
  nav: NavGroup[];
  titles: Record<string, string>;
}) {
  const pathname = usePathname();
  const [open, setOpen] = useState(false);

  return (
    <>
      <button type="button" className="sidebar-toggle" onClick={() => setOpen(!open)}>
        {open ? '× close' : '≡ menu'}
      </button>
      <aside className={`sidebar${open ? ' sidebar-open' : ''}`}>
        <nav aria-label="Documentation">
          {nav.map((group, gi) => (
            <div className="nav-group" key={group.label}>
              <span className="nav-group-label">
                <span className="nav-group-num">{String(gi + 1).padStart(2, '0')}</span>
                {group.label}
              </span>
              <ul>
                {group.slugs.map((slug) => {
                  const href = `/docs/${slug}`;
                  const active = pathname === href || pathname === `${href}/`;
                  return (
                    <li key={slug}>
                      <Link
                        href={href}
                        className={active ? 'nav-link nav-link-active' : 'nav-link'}
                        onClick={() => setOpen(false)}
                      >
                        {titles[slug] ?? slug}
                      </Link>
                    </li>
                  );
                })}
              </ul>
            </div>
          ))}
        </nav>
      </aside>
    </>
  );
}
