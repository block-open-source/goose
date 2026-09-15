#!/usr/bin/env node

// Generates an electron-updater Linux manifest listing the .deb and .rpm for
// one architecture. DebUpdater and RpmUpdater read the same file and each pick
// the entry with their own extension (Provider.js#findFile).
//
// Output name follows Provider.js#getChannelFilePrefix:
//   <channel>-linux.yml (x64) or <channel>-linux-arm64.yml (arm64)
// The channel defaults to "latest"; package variants that share a package name
// (vulkan) get their own channel so they are never updated across variants.
//
// Usage:
//   node scripts/generate-linux-update-manifest.js --version 1.49.0 --arch x64 \
//     --deb goose_1.49.0_amd64.deb --rpm Goose-1.49.0-1.x86_64.rpm

const crypto = require('node:crypto');
const fs = require('node:fs');
const path = require('node:path');

const SUPPORTED_ARCHES = ['x64', 'arm64'];

function manifestName(channel, arch) {
  const archSuffix = arch === 'x64' ? '' : `-${arch}`;
  return `${channel}-linux${archSuffix}.yml`;
}

function usage() {
  console.error(
    [
      'Usage: node scripts/generate-linux-update-manifest.js',
      '  --version <semver>',
      '  [--arch x64|arm64]        (defaults to "x64")',
      '  [--channel <name>]        (defaults to "latest")',
      '  [--deb <path>]...         (one or more .deb assets to include)',
      '  [--rpm <path>]...         (one or more .rpm assets to include)',
      '  [--out <manifest.yml>]    (defaults to <channel>-linux[-arm64].yml in cwd)',
    ].join('\n')
  );
}

function parseArgs(argv) {
  const args = {
    version: '',
    arch: 'x64',
    channel: 'latest',
    debs: [],
    rpms: [],
    out: '',
  };
  for (let i = 0; i < argv.length; i += 1) {
    const a = argv[i];
    if (a === '--version') args.version = argv[++i] || '';
    else if (a === '--arch') args.arch = argv[++i] || 'x64';
    else if (a === '--channel') args.channel = argv[++i] || 'latest';
    else if (a === '--deb' || a === '--rpm') {
      const value = argv[++i];
      if (!value) throw new Error(`${a} requires a file path`);
      (a === '--deb' ? args.debs : args.rpms).push(value);
    } else if (a === '--out') args.out = argv[++i] || '';
    else {
      usage();
      process.exit(1);
    }
  }
  if (!args.version) {
    usage();
    process.exit(1);
  }
  args.version = args.version.replace(/^v/, '');
  if (!SUPPORTED_ARCHES.includes(args.arch)) {
    throw new Error(
      `Unsupported arch "${args.arch}" — expected one of: ${SUPPORTED_ARCHES.join(', ')}`
    );
  }
  if (!/^[a-z0-9]+$/.test(args.channel)) {
    throw new Error(`Invalid channel "${args.channel}" — use lowercase letters and digits only`);
  }
  return args;
}

function sha512B64(filePath) {
  return crypto.createHash('sha512').update(fs.readFileSync(filePath)).digest('base64');
}

function yamlString(value) {
  return JSON.stringify(value);
}

function entryFor(filePath) {
  const p = path.resolve(filePath);
  if (!fs.existsSync(p)) throw new Error(`Missing artifact: ${p}`);
  const ext = path.extname(p).slice(1).toLowerCase();
  if (ext !== 'deb' && ext !== 'rpm') {
    throw new Error(`Expected a .deb or .rpm artifact, got: ${path.basename(p)}`);
  }
  const stats = fs.statSync(p);
  return {
    url: path.basename(p),
    sha512: sha512B64(p),
    size: stats.size,
  };
}

function writeManifest({ version, arch, channel, debs, rpms, out }) {
  if (debs.length === 0 && rpms.length === 0) {
    throw new Error('Provide at least one --deb or --rpm artifact');
  }
  const files = [...debs.map(entryFor), ...rpms.map(entryFor)];
  const primary = files[0];

  const lines = [
    `version: ${yamlString(version)}`,
    'files:',
    ...files.flatMap((f) => [
      `  - url: ${yamlString(f.url)}`,
      `    sha512: ${yamlString(f.sha512)}`,
      `    size: ${f.size}`,
    ]),
    // Legacy top-level path/sha512, kept for parity with the mac manifest.
    `path: ${yamlString(primary.url)}`,
    `sha512: ${yamlString(primary.sha512)}`,
    `releaseDate: ${yamlString(new Date().toISOString())}`,
    '',
  ];
  const text = lines.join('\n');

  const outPath = path.resolve(out || manifestName(channel, arch));
  fs.mkdirSync(path.dirname(outPath), { recursive: true });
  fs.writeFileSync(outPath, text);
  console.log(`Wrote ${outPath} for version ${version} (${files.length} artifact(s))`);
  for (const f of files) console.log(`  - ${f.url}  ${f.size} bytes`);
}

try {
  const args = parseArgs(process.argv.slice(2));
  writeManifest(args);
} catch (err) {
  console.error(err instanceof Error ? err.message : err);
  process.exit(1);
}
