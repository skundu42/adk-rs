import type { Metadata } from 'next';
import Link from 'next/link';
import '@fontsource-variable/bricolage-grotesque';
import '@fontsource-variable/jetbrains-mono';
import '@fontsource-variable/newsreader';
import '@fontsource-variable/newsreader/wght-italic.css';
import './globals.css';
import { CRATE_VERSION, REPO_URL } from '@/lib/registry';

export const metadata: Metadata = {
  title: {
    default: 'adk-rs — Agent Development Kit for Rust',
    template: '%s · adk-rs docs',
  },
  description:
    'Documentation for adk-rs: a code-first Rust framework for building, evaluating, and deploying AI agents.',
};

export default function RootLayout({ children }: { children: React.ReactNode }) {
  return (
    <html lang="en">
      <body>
        <header className="site-header">
          <Link href="/" className="wordmark">
            <span>
              adk<span className="tilde">::</span>rs
            </span>
            <span className="version-chip">v{CRATE_VERSION}</span>
          </Link>
          <nav className="header-nav">
            <Link href="/docs/introduction">Docs</Link>
            <Link href="/docs/examples/gemini-chat">Examples</Link>
            <Link href="/docs/guides/multi-agent-pipeline">Guides</Link>
            <a className="gh-link" href={REPO_URL} target="_blank" rel="noreferrer">
              GitHub ↗
            </a>
          </nav>
        </header>
        {children}
        <footer className="site-footer">
          <span>adk-rs · Apache-2.0 · Rust 1.85+ / edition 2024</span>
          <span>
            <a href="https://crates.io/crates/adk-rs" target="_blank" rel="noreferrer">
              crates.io/crates/adk-rs
            </a>
          </span>
        </footer>
      </body>
    </html>
  );
}
