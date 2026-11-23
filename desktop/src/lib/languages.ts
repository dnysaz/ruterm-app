/**
 * Maps a remote file path to a Monaco language id so the editor colours it.
 * Unknown files fall back to plain text (still editable, just uncoloured).
 */
const BY_EXTENSION: Record<string, string> = {
  js: 'javascript',
  mjs: 'javascript',
  cjs: 'javascript',
  jsx: 'javascript',
  ts: 'typescript',
  mts: 'typescript',
  cts: 'typescript',
  tsx: 'typescript',
  json: 'json',
  jsonc: 'json',
  html: 'html',
  htm: 'html',
  xhtml: 'html',
  vue: 'html',
  svelte: 'html',
  css: 'css',
  scss: 'scss',
  sass: 'scss',
  less: 'less',
  php: 'php',
  phtml: 'php',
  py: 'python',
  pyw: 'python',
  pyi: 'python',
  go: 'go',
  rs: 'rust',
  rb: 'ruby',
  rake: 'ruby',
  java: 'java',
  kt: 'kotlin',
  kts: 'kotlin',
  scala: 'scala',
  c: 'c',
  h: 'c',
  cc: 'cpp',
  cpp: 'cpp',
  cxx: 'cpp',
  hpp: 'cpp',
  hh: 'cpp',
  cs: 'csharp',
  swift: 'swift',
  m: 'objective-c',
  mm: 'objective-c',
  sh: 'shell',
  bash: 'shell',
  zsh: 'shell',
  fish: 'shell',
  ps1: 'powershell',
  bat: 'bat',
  cmd: 'bat',
  sql: 'sql',
  mysql: 'mysql',
  pgsql: 'pgsql',
  md: 'markdown',
  markdown: 'markdown',
  mdx: 'mdx',
  yml: 'yaml',
  yaml: 'yaml',
  toml: 'ini',
  ini: 'ini',
  conf: 'ini',
  cfg: 'ini',
  properties: 'ini',
  env: 'ini',
  xml: 'xml',
  svg: 'xml',
  plist: 'xml',
  lua: 'lua',
  pl: 'perl',
  pm: 'perl',
  r: 'r',
  dart: 'dart',
  ex: 'elixir',
  exs: 'elixir',
  erl: 'plaintext',
  hs: 'plaintext',
  clj: 'clojure',
  coffee: 'coffeescript',
  graphql: 'graphql',
  gql: 'graphql',
  proto: 'protobuf',
  tf: 'hcl',
  tfvars: 'hcl',
  hcl: 'hcl',
  gradle: 'plaintext',
  twig: 'twig',
  pug: 'pug',
  jade: 'pug',
  liquid: 'liquid',
  sol: 'solidity',
  vb: 'vb',
  f90: 'plaintext',
};

/** Whole filenames that have nothing (or no useful) extension. */
const BY_NAME: Record<string, string> = {
  dockerfile: 'dockerfile',
  'docker-compose.yml': 'yaml',
  'docker-compose.yaml': 'yaml',
  makefile: 'makefile',
  gnumakefile: 'makefile',
  rakefile: 'ruby',
  gemfile: 'ruby',
  procfile: 'plaintext',
  '.bashrc': 'shell',
  '.bash_profile': 'shell',
  '.zshrc': 'shell',
  '.zprofile': 'shell',
  '.profile': 'shell',
  '.gitignore': 'plaintext',
  '.gitattributes': 'plaintext',
  '.editorconfig': 'ini',
  '.npmrc': 'ini',
  '.env': 'ini',
  'cargo.lock': 'ini',
  'package-lock.json': 'json',
};

export function languageForPath(path: string): string {
  const name = path.slice(path.lastIndexOf('/') + 1).toLowerCase();
  if (!name) return 'plaintext';
  const byName = BY_NAME[name];
  if (byName) return byName;
  const dot = name.lastIndexOf('.');
  if (dot <= 0) return 'plaintext';
  return BY_EXTENSION[name.slice(dot + 1)] ?? 'plaintext';
}

/** Whole label aliases used by fenced code blocks in notes and AI chat. */
const LABEL_ALIASES: Record<string, string> = {
  text: 'plaintext',
  txt: 'plaintext',
  code: 'plaintext',
  console: 'plaintext',
  log: 'plaintext',
  shellsession: 'shell',
  shell: 'shell',
  bash: 'shell',
  zsh: 'shell',
  sh: 'shell',
};

/** Map a fenced-code label (e.g. `bash`, `ts`) to a Monaco language id. */
export function languageForLabel(label: string): string {
  const key = label.trim().toLowerCase().replace(/^\./, '');
  if (!key) return 'plaintext';
  return LABEL_ALIASES[key] ?? BY_EXTENSION[key] ?? key;
}
