// SPDX-License-Identifier: Apache-2.0
import { randomUUID } from 'node:crypto';
import { existsSync, mkdirSync, readdirSync, readFileSync, statSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import Ajv from 'ajv';
import addFormats from 'ajv-formats';

const here = dirname(fileURLToPath(import.meta.url));
const rootDir = resolve(here, '..');

export interface SbomPackage {
  ecosystem: 'cargo' | 'npm';
  name: string;
  version: string;
  purl: string;
  license?: string;
  source?: string;
  checksum?: string;
}

export function getReleaseVersion(): string {
  const cargoPath = join(rootDir, 'Cargo.toml');
  if (existsSync(cargoPath)) {
    const cargoToml = readFileSync(cargoPath, 'utf8');
    const m = cargoToml.match(/\[workspace\.package\][^[]*?version\s*=\s*"([^"]+)"/s);
    if (m) return m[1];
  }
  const cliPkgPath = join(rootDir, 'apps/cli/package.json');
  if (existsSync(cliPkgPath)) {
    const pkg = JSON.parse(readFileSync(cliPkgPath, 'utf8'));
    if (pkg.version) return pkg.version;
  }
  return '0.2.0';
}

export function parseCargoLock(cargoLockContent: string): SbomPackage[] {
  const packages: SbomPackage[] = [];
  const blocks = cargoLockContent.split('[[package]]');

  for (const block of blocks) {
    const nameMatch = block.match(/name\s*=\s*"([^"]+)"/);
    const versionMatch = block.match(/version\s*=\s*"([^"]+)"/);
    const sourceMatch = block.match(/source\s*=\s*"([^"]+)"/);
    const checksumMatch = block.match(/checksum\s*=\s*"([^"]+)"/);

    if (nameMatch && versionMatch) {
      const name = nameMatch[1];
      const version = versionMatch[1];
      // Filter out root workspace members
      if (
        name === 'tokentree-cli' ||
        name === 'tokentree-core' ||
        name === 'tokentree-ledger' ||
        name === 'tokentree-codex' ||
        name === 'tokentree-claude' ||
        name === 'tokentree-otel'
      ) {
        continue;
      }

      packages.push({
        ecosystem: 'cargo',
        name,
        version,
        purl: `pkg:cargo/${name}@${version}`,
        source: sourceMatch ? sourceMatch[1] : undefined,
        checksum: checksumMatch ? checksumMatch[1] : undefined,
      });
    }
  }

  return packages;
}

export function collectNpmPackages(releaseVersion: string): SbomPackage[] {
  const packages: SbomPackage[] = [];
  const seen = new Set<string>();

  const scanDirs = [
    join(rootDir, 'apps'),
    join(rootDir, 'packages'),
    join(rootDir, 'packages/adapters'),
    join(rootDir, 'plugins'),
  ];

  for (const parent of scanDirs) {
    if (!existsSync(parent)) continue;
    for (const entry of readdirSync(parent)) {
      const pkgJsonPath = join(parent, entry, 'package.json');
      if (existsSync(pkgJsonPath) && statSync(pkgJsonPath).isFile()) {
        try {
          const pkg = JSON.parse(readFileSync(pkgJsonPath, 'utf8'));
          if (pkg.name) {
            const version = pkg.version === '0.0.0' ? releaseVersion : pkg.version || releaseVersion;
            const key = `${pkg.name}@${version}`;
            if (!seen.has(key)) {
              seen.add(key);
              let purl: string;
              if (pkg.name.startsWith('@')) {
                const [scope, unscoped] = pkg.name.split('/');
                purl = `pkg:npm/%40${scope.slice(1)}/${unscoped}@${version}`;
              } else {
                purl = `pkg:npm/${pkg.name}@${version}`;
              }

              packages.push({
                ecosystem: 'npm',
                name: pkg.name,
                version,
                purl,
                license: pkg.license || 'Apache-2.0',
              });
            }

            // Also check declared production dependencies
            if (pkg.dependencies && typeof pkg.dependencies === 'object') {
              for (const [depName, depVer] of Object.entries(pkg.dependencies)) {
                if (typeof depVer === 'string' && !depVer.startsWith('workspace:')) {
                  const cleanVer = depVer.replace(/^[\^~]/, '');
                  const depKey = `${depName}@${cleanVer}`;
                  if (!seen.has(depKey)) {
                    seen.add(depKey);
                    let purl: string;
                    if (depName.startsWith('@')) {
                      const [scope, unscoped] = depName.split('/');
                      purl = `pkg:npm/%40${scope.slice(1)}/${unscoped}@${cleanVer}`;
                    } else {
                      purl = `pkg:npm/${depName}@${cleanVer}`;
                    }
                    packages.push({
                      ecosystem: 'npm',
                      name: depName,
                      version: cleanVer,
                      purl,
                      license: 'NOASSERTION',
                    });
                  }
                }
              }
            }
          }
        } catch {
          // ignore unparseable package.json
        }
      }
    }
  }

  return packages;
}

export function generateSbom(outputDir: string) {
  mkdirSync(outputDir, { recursive: true });

  const releaseVersion = getReleaseVersion();
  const cargoLockPath = join(rootDir, 'Cargo.lock');
  let cargoPackages: SbomPackage[] = [];
  if (existsSync(cargoLockPath)) {
    const lockContent = readFileSync(cargoLockPath, 'utf8');
    cargoPackages = parseCargoLock(lockContent);
  }

  const npmPackages = collectNpmPackages(releaseVersion);
  const allPackages = [...cargoPackages, ...npmPackages];

  const timestamp = new Date().toISOString();

  // 1. Generate SPDX 2.3 JSON
  const spdxPackages = [
    {
      SPDXID: 'SPDXRef-Package-tokentree',
      name: 'tokentree',
      versionInfo: releaseVersion,
      downloadLocation: 'git+https://github.com/tokentreehq/tokentree.git',
      filesAnalyzed: false,
      licenseConcluded: 'Apache-2.0',
      licenseDeclared: 'Apache-2.0',
      supplier: 'Organization: tokentreehq',
      externalRefs: [
        {
          referenceCategory: 'PACKAGE-MANAGER',
          referenceType: 'purl',
          referenceLocator: `pkg:cargo/tokentree@${releaseVersion}`,
        },
      ],
    },
  ];

  const spdxRelationships = [
    {
      spdxElementId: 'SPDXRef-DOCUMENT',
      relatedSpdxElement: 'SPDXRef-Package-tokentree',
      relationshipType: 'DESCRIBES',
    },
  ];

  for (const pkg of allPackages) {
    const cleanName = pkg.name.replace(/[^a-zA-Z0-9-]/g, '-');
    const cleanVer = pkg.version.replace(/[^a-zA-Z0-9-]/g, '-');
    const spdxId = `SPDXRef-Package-${pkg.ecosystem}-${cleanName}-${cleanVer}`;

    spdxPackages.push({
      SPDXID: spdxId,
      name: pkg.name,
      versionInfo: pkg.version,
      downloadLocation: 'NOASSERTION',
      filesAnalyzed: false,
      licenseConcluded: pkg.license || 'NOASSERTION',
      licenseDeclared: pkg.license || 'NOASSERTION',
      supplier: 'NOASSERTION',
      externalRefs: [
        {
          referenceCategory: 'PACKAGE-MANAGER',
          referenceType: 'purl',
          referenceLocator: pkg.purl,
        },
      ],
    });

    spdxRelationships.push({
      spdxElementId: 'SPDXRef-Package-tokentree',
      relatedSpdxElement: spdxId,
      relationshipType: 'DEPENDS_ON',
    });
  }

  const spdxDoc = {
    spdxVersion: 'SPDX-2.3',
    dataLicense: 'CC0-1.0',
    SPDXID: 'SPDXRef-DOCUMENT',
    name: 'tokentree',
    documentNamespace: `https://github.com/tokentreehq/tokentree/spdx/tokentree-${releaseVersion}-${Date.now()}`,
    creationInfo: {
      created: timestamp,
      creators: ['Tool: tokentree-sbom-generator-1.0', 'Organization: tokentreehq'],
    },
    packages: spdxPackages,
    relationships: spdxRelationships,
  };

  const spdxPath = join(outputDir, 'tokentree-spdx-sbom.json');
  writeFileSync(spdxPath, JSON.stringify(spdxDoc, null, 2), 'utf8');

  // 2. Generate CycloneDX 1.5 JSON
  const cyclonedxComponents = allPackages.map((pkg) => ({
    type: 'library',
    name: pkg.name,
    version: pkg.version,
    purl: pkg.purl,
    'bom-ref': pkg.purl,
  }));

  const cyclonedxDoc = {
    $schema: 'http://cyclonedx.org/schema/bom-1.5.schema.json',
    bomFormat: 'CycloneDX',
    specVersion: '1.5',
    serialNumber: `urn:uuid:${randomUUID()}`,
    version: 1,
    metadata: {
      timestamp,
      tools: [
        {
          vendor: 'tokentreehq',
          name: 'tokentree-sbom-generator',
          version: '1.0.0',
        },
      ],
      component: {
        type: 'application',
        name: 'tokentree',
        version: releaseVersion,
        purl: `pkg:cargo/tokentree@${releaseVersion}`,
        licenses: [
          {
            license: {
              id: 'Apache-2.0',
            },
          },
        ],
      },
    },
    components: cyclonedxComponents,
  };

  const cyclonedxPath = join(outputDir, 'tokentree-cyclonedx-sbom.json');
  writeFileSync(cyclonedxPath, JSON.stringify(cyclonedxDoc, null, 2), 'utf8');

  console.log(`[SBOM] Generated SPDX 2.3 SBOM at: ${spdxPath} (${spdxPackages.length} packages, version ${releaseVersion})`);
  console.log(`[SBOM] Generated CycloneDX 1.5 SBOM at: ${cyclonedxPath} (${cyclonedxComponents.length} components, version ${releaseVersion})`);
}

export function validateSbomFiles(outputDir: string): { spdxValid: boolean; cyclonedxValid: boolean } {
  const schemasDir = join(rootDir, 'schemas');
  const spdxSchemaPath = join(schemasDir, 'spdx-2.3.schema.json');
  const cdxSchemaPath = join(schemasDir, 'cyclonedx-1.5.schema.json');
  const cdxSpdxPath = join(schemasDir, 'spdx.schema.json');
  const jsfSchemaPath = join(schemasDir, 'jsf-0.82.schema.json');

  if (!existsSync(spdxSchemaPath) || !existsSync(cdxSchemaPath)) {
    throw new Error(`Official schemas missing in ${schemasDir}`);
  }

  const ajv = new Ajv({ allErrors: true, strict: false });
  addFormats(ajv);
  ajv.addFormat('iri-reference', true);
  ajv.addFormat('idn-email', true);

  if (existsSync(cdxSpdxPath)) {
    ajv.addSchema(JSON.parse(readFileSync(cdxSpdxPath, 'utf8')), 'spdx.schema.json');
  }
  if (existsSync(jsfSchemaPath)) {
    ajv.addSchema(JSON.parse(readFileSync(jsfSchemaPath, 'utf8')), 'jsf-0.82.schema.json');
  }

  const spdxSchema = JSON.parse(readFileSync(spdxSchemaPath, 'utf8'));
  const validateSpdx = ajv.compile(spdxSchema);

  const spdxDoc = JSON.parse(readFileSync(join(outputDir, 'tokentree-spdx-sbom.json'), 'utf8'));
  const spdxValid = Boolean(validateSpdx(spdxDoc));
  if (!spdxValid) {
    console.error('[SBOM Validation Error] SPDX 2.3 schema errors:', validateSpdx.errors);
    throw new Error(`SPDX 2.3 schema validation failed: ${JSON.stringify(validateSpdx.errors)}`);
  }

  const cdxSchema = JSON.parse(readFileSync(cdxSchemaPath, 'utf8'));
  const validateCdx = ajv.compile(cdxSchema);

  const cdxDoc = JSON.parse(readFileSync(join(outputDir, 'tokentree-cyclonedx-sbom.json'), 'utf8'));
  const cyclonedxValid = Boolean(validateCdx(cdxDoc));
  if (!cyclonedxValid) {
    console.error('[SBOM Validation Error] CycloneDX 1.5 schema errors:', validateCdx.errors);
    throw new Error(`CycloneDX 1.5 schema validation failed: ${JSON.stringify(validateCdx.errors)}`);
  }

  console.log('[SBOM Validation] SPDX 2.3 and CycloneDX 1.5 both validated successfully against official schemas.');
  return { spdxValid, cyclonedxValid };
}

if (process.argv[1] && process.argv[1].endsWith('generate-sbom.ts')) {
  const targetDir = process.argv[2] || resolve(rootDir, 'release-assets');
  generateSbom(targetDir);
  validateSbomFiles(targetDir);
}
