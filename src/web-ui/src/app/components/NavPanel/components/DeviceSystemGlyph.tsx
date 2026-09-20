import { Monitor } from 'lucide-react';
import type { DeviceOverviewDevice } from '../deviceInterconnectionOverview';
import { getDeviceArtworkKind } from './deviceArtworkKind';
import { DEVICE_MARK_BY_ARTWORK, DEVICE_SYSTEM_MARKS } from './deviceSystemMarks';

type DeviceSystemFacts = Pick<DeviceOverviewDevice, 'kind' | 'name' | 'os' | 'hostKind'>;

/**
 * The mark a device row shows in front of its name: the system the device
 * reported, the server silhouette for a CLI host, and the neutral monitor this
 * list always drew for a device whose system cannot be placed.
 *
 * An inline path rather than a masked asset file, because a mask whose asset
 * does not resolve paints the element as a solid block instead of falling back
 * to no mark, and the inline form needs no mask, size, or colour rules.
 */
export function DeviceSystemGlyph({ device, size = 16 }: { device: DeviceSystemFacts; size?: number }) {
  const key = DEVICE_MARK_BY_ARTWORK[getDeviceArtworkKind(device)];
  if (!key) return <Monitor size={size} />;
  const mark = DEVICE_SYSTEM_MARKS[key];
  return (
    <svg
      aria-hidden="true"
      data-system={key}
      focusable="false"
      height={size}
      viewBox={mark.viewBox}
      width={size}
      xmlns="http://www.w3.org/2000/svg"
    >
      <path fill="currentColor" d={mark.path} />
    </svg>
  );
}
