// N-12.3: regenerate the npm third-party license notices bundled with the app.
//
// Reads every installed frontend dependency via the `license-checker`
// devDependency, normalizes the result, inlines each distinct license file
// text once, and writes
//     crates/tauri-app/resources/licenses/npm-licenses.json
// (only when the content actually changed, so downstream builds don't loop).
//
// Wired into the build via frontend/package.json `pregenerate`, which runs
// before every `nuxt generate` — including Tauri's `beforeBuildCommand`.
// The Rust half (cargo-about) runs from crates/tauri-app/build.rs.
//
// The script never fails the build: if `license-checker` is not installed
// (e.g. `npm ci` hasn't run) it warns on stderr and keeps the checked-in
// file, which is the fallback the app bundles.

import { createRequire } from 'node:module'
import { createHash } from 'node:crypto'
import { readFileSync, writeFileSync, mkdirSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const scriptDir = dirname(fileURLToPath(import.meta.url))
const repoRoot = dirname(scriptDir)
const frontendDir = join(repoRoot, 'frontend')
const outDir = join(repoRoot, 'crates', 'tauri-app', 'resources', 'licenses')
const outFile = join(outDir, 'npm-licenses.json')

function warn(msg) {
  console.error(`[generate-licenses] warning: ${msg}`)
}

let checker
try {
  const require = createRequire(join(frontendDir, 'package.json'))
  checker = require('license-checker')
} catch (e) {
  warn(`license-checker is not installed (${e.message}); keeping checked-in npm-licenses.json`)
  process.exit(0)
}

let generatorVersion = 'unknown'
try {
  const installed = JSON.parse(readFileSync(join(frontendDir, 'node_modules', 'license-checker', 'package.json'), 'utf8'))
  if (installed.version) generatorVersion = installed.version
} catch {
  const pkgJson = JSON.parse(readFileSync(join(frontendDir, 'package.json'), 'utf8'))
  generatorVersion = pkgJson.devDependencies?.['license-checker'] ?? 'unknown'
}

function asLicenseList(value) {
  if (Array.isArray(value)) return value.map(String)
  if (typeof value === 'string' && value.length > 0) return [value]
  return []
}

function initChecker(opts) {
  return new Promise((resolve, reject) => {
    checker.init(opts, (err, packages) => (err ? reject(err) : resolve(packages)))
  })
}

function splitNameVersion(key) {
  // Keys look like "name@version" or "@scope/name@version".
  const at = key.lastIndexOf('@')
  if (at <= 0) return { name: key, version: '' }
  return { name: key.slice(0, at), version: key.slice(at + 1) }
}

const raw = await initChecker({ start: frontendDir }).catch((e) => {
  warn(`license-checker failed (${e.message}); keeping checked-in npm-licenses.json`)
  process.exit(0)
})

const packages = []
// Deduplicate license texts by content hash: hundreds of packages share the
// same MIT text, and we only want to bundle it once.
const textsByHash = new Map()

for (const [key, info] of Object.entries(raw)) {
  const { name, version } = splitNameVersion(key)
  const licenses = asLicenseList(info.licenses)
  packages.push({
    name,
    version,
    licenses,
    repository: info.repository || null,
    publisher: info.publisher || null,
  })

  const licenseFile = info.licenseFile
  if (licenseFile) {
    let text = null
    try {
      text = readFileSync(licenseFile, 'utf8')
    } catch {
      // Unreadable license file: the package row still lists its license
      // expression; there is just no text to bundle for it.
    }
    if (text && text.trim().length > 0) {
      const hash = createHash('sha256').update(text).digest('hex')
      let entry = textsByHash.get(hash)
      if (!entry) {
        entry = { licenses: licenses.join(' OR ') || 'UNKNOWN', text, packages: [] }
        textsByHash.set(hash, entry)
      }
      entry.packages.push(`${name}@${version}`)
    }
  }
}

packages.sort((a, b) => a.name.localeCompare(b.name) || a.version.localeCompare(b.version))
const licenseTexts = [...textsByHash.values()].sort((a, b) => a.licenses.localeCompare(b.licenses))
for (const entry of licenseTexts) entry.packages.sort()

const doc = {
  generated_by: `license-checker ${generatorVersion}`,
  packages,
  license_texts: licenseTexts,
}

const serialized = JSON.stringify(doc, null, 2) + '\n'
mkdirSync(outDir, { recursive: true })
let current = null
try {
  current = readFileSync(outFile, 'utf8')
} catch {
  // First generation: no checked-in file yet.
}
if (current !== serialized) {
  writeFileSync(outFile, serialized)
  console.error(`[generate-licenses] wrote ${outFile} (${packages.length} packages, ${licenseTexts.length} distinct license texts)`)
} else {
  console.error('[generate-licenses] npm-licenses.json unchanged')
}
