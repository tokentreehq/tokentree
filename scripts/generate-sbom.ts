// SPDX-License-Identifier: Apache-2.0
import { randomUUID } from 'node:crypto';
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const rootDir = resolve(here, '..');

interface CargoPackage {
  name: string;
  version: string;
  source?: string;
  checksum?: string;
}

function parseCargoLock(cargoLockContent: string): CargoPackage[] {
  const packages: CargoPackage[] = [];
  const blocks = cargoLockContent.split('[[package]]');

  for (const block of blocks) {
    const nameMatch = block.match(/name\s*=\s*"([^"]+)"/);
    const versionMatch = block.match(/version\s*=\s*"([^"]+)"/);
    const sourceMatch = block.match(/source\s*=\s*"([^"]+)"/);
    const checksumMatch = block.match(/checksum\s*=\s*"([^"]+)"/);

    if (nameMatch && versionMatch) {
      packages.push({
        name: nameMatch[1],
        version: versionMatch[1],
        source: sourceMatch ? sourceMatch[1] : undefined,
        checksum: checksumMatch ? checksumMatch[1] : undefined,
      });
    }
  }

  return packages;
}

export function generateSbom(outputDir: string) {
  mkdirSync(outputDir, { recursive: true });

  const cargoLockPath = join(rootDir, 'Cargo.lock');
  let cargoPackages: CargoPackage[] = [];
  if (existsSync(cargoLockPath)) {
    const lockContent = readFileSync(cargoLockPath, 'utf8');
    cargoPackages = parseCargoLock(lockContent);
  }

  const timestamp = new Date().toISOString();
  const rootVersion = '0.2.0';

  // 1. Generate SPDX 2.3 JSON
  const spdxPackages = [
    {
      SPDXID: 'SPDXRef-Package-tokentree',
      name: 'tokentree',
      versionInfo: rootVersion,
      downloadLocation: 'git+https://github.com/tokentreehq/tokentree.git',
      filesAnalyzed: false,
      licenseConcluded: 'Apache-2.0',
      licenseDeclared: 'Apache-2.0',
      supplier: 'Organization: tokentreehq',
      externalRefs: [
        {
          referenceCategory: 'PACKAGE-MANAGER',
          referenceType: 'purl',
          referenceLocator: `pkg:cargo/tokentree@${rootVersion}`,
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

  for (const pkg of cargoPackages) {
    if (pkg.name === 'tokentree-cli' || pkg.name === 'tokentree-core' || pkg.name === 'tokentree-ledger') {
      continue;
    }
    const cleanName = pkg.name.replace(/[^a-zA-Z0-9-]/g, '-');
    const spdxId = `SPDXRef-Package-cargo-${cleanName}-${pkg.version.replace(/[^a-zA-Z0-9-]/g, '-')}`;
    spdxPackages.push({
      SPDXID: spdxId,
      name: pkg.name,
      versionInfo: pkg.version,
      downloadLocation: 'NOASSERTION',
      filesAnalyzed: false,
      licenseConcluded: 'NOASSERTION',
      licenseDeclared: 'NOASSERTION',
      supplier: 'NOASSERTION',
      externalRefs: [
        {
          referenceCategory: 'PACKAGE-MANAGER',
          referenceType: 'purl',
          referenceLocator: `pkg:cargo/${pkg.name}@${pkg.version}`,
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
    documentNamespace: `https://github.com/tokentreehq/tokentree/spdx/tokentree-${rootVersion}-${Date.now()}`,
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
  const cyclonedxComponents = cargoPackages
    .filter((p) => p.name !== 'tokentree-cli' && p.name !== 'tokentree-core' && p.name !== 'tokentree-ledger')
    .map((pkg) => ({
      type: 'library',
      name: pkg.name,
      version: pkg.version,
      purl: `pkg:cargo/${pkg.name}@${pkg.version}`,
      'bom-ref': `pkg:cargo/${pkg.name}@${pkg.version}`,
    }));

  const cyclonedxDoc = {
    $schema: 'http://cyclonedx.org/schema/bom-1.5.json',
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
        version: rootVersion,
        purl: `pkg:cargo/tokentree@${rootVersion}`,
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

  console.log(`[SBOM] Generated SPDX 2.3 SBOM at: ${spdxPath} (${spdxPackages.length} packages)`);
  console.log(`[SBOM] Generated CycloneDX 1.5 SBOM at: ${cyclonedxPath} (${cyclonedxComponents.length} components)`);
}

const targetDir = process.argv[2] || resolve(rootDir, 'release-assets');
generateSbom(targetDir);
