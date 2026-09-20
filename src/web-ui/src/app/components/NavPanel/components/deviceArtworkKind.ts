import type { DeviceOverviewDevice } from '../deviceInterconnectionOverview';

export type DeviceArtworkKind =
  | 'device'
  | 'server'
  | 'macbook-air'
  | 'windows'
  | 'macos'
  | 'linux'
  | 'harmonyos';

type DeviceArtworkFacts = Pick<DeviceOverviewDevice, 'kind' | 'name' | 'os' | 'hostKind'>;

/**
 * Artwork for the system a device reported.
 *
 * OpenBitFun hosts report `macOS`, `Windows`, `Linux` and `HarmonyOS`, but the
 * same field is filled in by any client and by older Relays, so the match stays
 * tolerant about casing and spelling. A system we cannot place keeps the
 * neutral artwork instead of claiming a plausible one.
 */
function systemArtworkKind(os: string | null | undefined): DeviceArtworkKind {
  const value = os?.trim().toLowerCase() ?? '';
  if (!value) return 'device';
  if (value.includes('harmony') || value.includes('ohos')) return 'harmonyos';
  if (value.startsWith('win') || value.includes('windows')) return 'windows';
  if (value.includes('mac') || value.includes('darwin') || value.includes('osx')) return 'macos';
  if (value.includes('linux')) return 'linux';
  return 'device';
}

export function getDeviceArtworkKind(device: DeviceArtworkFacts): DeviceArtworkKind {
  // A CLI host has no window to draw, whatever machine it runs on.
  if (device.hostKind === 'cli') return 'server';
  if (device.kind === 'execution-host') return 'server';
  if (device.kind !== 'desktop') return 'device';
  // Device names can identify a model, but the controller's OS cannot identify
  // a remote machine. Unrecognized names deliberately use neutral artwork.
  if (/\bmacbook[\s._-]*air\b/i.test(device.name)) return 'macbook-air';
  return systemArtworkKind(device.os);
}
