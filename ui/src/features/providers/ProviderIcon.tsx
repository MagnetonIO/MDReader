import type { SVGProps } from 'react';
import { DocIcon, DropboxIcon, GoogleDriveIcon } from '../../shell/icons';

/** Local brand artwork; surrounding text supplies the accessible provider name. */
export function ProviderIcon({ provider, ...props }: SVGProps<SVGSVGElement> & { provider: string }) {
  const Icon = provider === 'google_drive' ? GoogleDriveIcon
    : provider === 'dropbox' ? DropboxIcon : DocIcon;
  return <Icon width={20} height={20} aria-hidden="true" {...props} />;
}
