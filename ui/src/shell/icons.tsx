/** Small, uniform inline SVG icons. `currentColor` throughout so they inherit the
 * quiet rail palette. Kept minimal — the chrome should not shout. */

import type { SVGProps } from 'react';

type IconProps = SVGProps<SVGSVGElement>;

function Icon({ children, ...props }: IconProps) {
  return (
    <svg
      width="16"
      height="16"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.7"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      focusable="false"
      {...props}
    >
      {children}
    </svg>
  );
}

export const ChevronRight = (props: IconProps) => (
  <Icon {...props}>
    <path d="M9 6l6 6-6 6" />
  </Icon>
);

export const DocIcon = (props: IconProps) => (
  <Icon {...props}>
    <path d="M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8z" />
    <path d="M14 3v5h5" />
  </Icon>
);

export const FolderIcon = (props: IconProps) => (
  <Icon {...props}>
    <path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z" />
  </Icon>
);

export const PanelLeft = (props: IconProps) => (
  <Icon {...props}>
    <rect x="3" y="4" width="18" height="16" rx="2" />
    <path d="M9 4v16" />
  </Icon>
);

export const ListIcon = (props: IconProps) => (
  <Icon {...props}>
    <path d="M8 6h13M8 12h13M8 18h13M3 6h.01M3 12h.01M3 18h.01" />
  </Icon>
);

export const CloseIcon = (props: IconProps) => (
  <Icon {...props}>
    <path d="M6 6l12 12M18 6L6 18" />
  </Icon>
);

export const PinIcon = (props: IconProps) => (
  <Icon width="12" height="12" {...props}>
    <path d="M12 3l3 6 5 1-4 4 1 6-5-3-5 3 1-6-4-4 5-1z" />
  </Icon>
);

export const FocusIcon = (props: IconProps) => (
  <Icon {...props}>
    <circle cx="12" cy="12" r="3" />
    <path d="M3 7V5a2 2 0 0 1 2-2h2M17 3h2a2 2 0 0 1 2 2v2M21 17v2a2 2 0 0 1-2 2h-2M7 21H5a2 2 0 0 1-2-2v-2" />
  </Icon>
);

export const TypeIcon = (props: IconProps) => (
  <Icon {...props}>
    <path d="M4 7V5h16v2M9 19h6M12 5v14" />
  </Icon>
);

export const ThemeIcon = (props: IconProps) => (
  <Icon {...props}>
    <path d="M12 3a9 9 0 1 0 9 9 7 7 0 0 1-9-9z" />
  </Icon>
);

export const SplitIcon = (props: IconProps) => (
  <Icon {...props}>
    <rect x="3" y="4" width="18" height="16" rx="2" />
    <path d="M12 4v16" />
  </Icon>
);
