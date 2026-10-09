import type { SVGProps } from 'react';

export function ActionIcon({ kind, ...props }: SVGProps<SVGSVGElement> & { kind: 'export' | 'share' | 'link' | 'bookmark' | 'resume' }) {
  return <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor"
    strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true" focusable="false" {...props}>
    {kind === 'export' ? <><path d="M12 3v12m-4-4 4 4 4-4" /><path d="M5 16v4h14v-4" /></> : null}
    {kind === 'share' ? <><path d="M12 15V3m-4 4 4-4 4 4" /><path d="M7 10H5v11h14V10h-2" /></> : null}
    {kind === 'link' ? <><path d="m10 13 4-4" /><path d="m8 16-1 1a4 4 0 0 1-6-6l4-4a4 4 0 0 1 6 0m2 1 1-1a4 4 0 0 1 6 6l-4 4a4 4 0 0 1-6 0" /></> : null}
    {kind === 'bookmark' ? <path d="M6 3h12v18l-6-4-6 4z" /> : null}
    {kind === 'resume' ? <><path d="M4 10a8 8 0 1 1 1 8M4 4v6h6" /><path d="M12 7v5l3 2" /></> : null}
  </svg>;
}
