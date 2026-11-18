// OS thumbnail metadata.
// Icons are official brand marks from Simple Icons (CC0), bundled in
// /static/os/*.svg. Unknown distros fall back to a letter monogram.

export interface OsLook {
  /** Two/three-letter glyph drawn when no official icon is available. */
  text: string;
  /** Human-readable name, used for tooltips. */
  name: string;
  /** Monogram disc colors (fallback only). */
  from: string;
  to: string;
  fg: string;
  /** File name in /static/os (without extension), or null for letters. */
  slug: string | null;
}

const TABLE: Record<string, OsLook> = {
  ubuntu: { text: 'Ub', name: 'Ubuntu', from: '#ec5a26', to: '#c34113', fg: '#fff', slug: 'ubuntu' },
  debian: { text: 'De', name: 'Debian', from: '#d70751', to: '#9b0040', fg: '#fff', slug: 'debian' },
  mint: { text: 'Mi', name: 'Linux Mint', from: '#86be43', to: '#52882a', fg: '#10230a', slug: 'mint' },
  pop: { text: 'Po', name: 'Pop!_OS', from: '#48b9c7', to: '#2a7d88', fg: '#062a2e', slug: 'pop' },
  elementary: { text: 'El', name: 'elementary OS', from: '#64baff', to: '#2f7fd6', fg: '#062033', slug: 'elementary' },
  cachyos: { text: 'Ca', name: 'CachyOS', from: '#00aa88', to: '#006b52', fg: '#06281f', slug: 'cachyos' },
  arch: { text: 'Ar', name: 'Arch Linux', from: '#1793d1', to: '#0d648f', fg: '#fff', slug: 'arch' },
  fedora: { text: 'Fe', name: 'Fedora', from: '#51a2da', to: '#20496f', fg: '#fff', slug: 'fedora' },
  rhel: { text: 'Rh', name: 'Red Hat Enterprise Linux', from: '#ee0000', to: '#8f0000', fg: '#fff', slug: 'rhel' },
  centos: { text: 'Ce', name: 'CentOS', from: '#262577', to: '#151538', fg: '#fff', slug: 'centos' },
  rocky: { text: 'Ro', name: 'Rocky Linux', from: '#10b981', to: '#0a7a55', fg: '#062a1f', slug: 'rocky' },
  alma: { text: 'Am', name: 'AlmaLinux', from: '#000000', to: '#2a2a2a', fg: '#fff', slug: 'alma' },
  suse: { text: 'Su', name: 'SUSE / openSUSE', from: '#73ba25', to: '#3d7a10', fg: '#0f2906', slug: 'suse' },
  alpine: { text: 'Al', name: 'Alpine Linux', from: '#0d597f', to: '#06324a', fg: '#fff', slug: 'alpine' },
  gentoo: { text: 'G', name: 'Gentoo', from: '#54487a', to: '#352a5e', fg: '#fff', slug: 'gentoo' },
  raspbian: { text: 'Rp', name: 'Raspberry Pi OS', from: '#a22846', to: '#6e1226', fg: '#fff', slug: 'raspbian' },
  kali: { text: 'Ka', name: 'Kali Linux', from: '#557c94', to: '#2f4a5c', fg: '#fff', slug: 'kali' },
  freebsd: { text: 'Fb', name: 'FreeBSD', from: '#ab2b28', to: '#611614', fg: '#fff', slug: 'freebsd' },
  macos: { text: 'Mac', name: 'macOS', from: '#5b6470', to: '#383e48', fg: '#fff', slug: 'macos' },
  windows: { text: '', name: 'Windows', from: '#0078d6', to: '#005a9e', fg: '#fff', slug: 'windows' },
  linux: { text: 'Li', name: 'Linux', from: '#fcc624', to: '#a8842e', fg: '#2b2103', slug: 'linux' },
  unknown: { text: '?', name: 'Unknown OS', from: '#5a5a57', to: '#3a3a37', fg: '#fff', slug: null },
};

/** Canonical id from the probe (anything else falls back to a gray monogram). */
export function osLook(os: string | null | undefined): OsLook {
  const key = (os ?? '').toLowerCase().replace(/[^a-z0-9]/g, '');
  if (TABLE[key]) return TABLE[key];
  if (key) {
    return {
      text: key.slice(0, 2).replace(/^./, c => c.toUpperCase()),
      name: (os ?? '')
        .split('_')
        .map(p => p.charAt(0).toUpperCase() + p.slice(1))
        .join(' '),
      from: '#5a5a57',
      to: '#3a3a37',
      fg: '#fff',
      slug: null,
    };
  }
  return TABLE.unknown;
}

export function osName(os: string | null | undefined): string {
  return osLook(os).name;
}
