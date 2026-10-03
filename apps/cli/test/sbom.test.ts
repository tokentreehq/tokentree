// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from 'vitest';
import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import {
  generateSbom,
  getReleaseVersion,
  validateSbomFiles,
} from '../../../scripts/generate-sbom.js';

describe('Software Bill of Materials (SBOM) generation & schema validation', () => {
  it('generates valid SPDX 2.3 and CycloneDX 1.5 documents with Rust and npm dependencies', () => {
    const tmp = mkdtempSync(join(tmpdir(), 'tokentree-sbom-test-'));
    try {
      const releaseVersion = getReleaseVersion();
      expect(releaseVersion).toBeTruthy();
      expect(typeof releaseVersion).toBe('string');

      generateSbom(tmp);

      // Validate against official schemas
      const validation = validateSbomFiles(tmp);
      expect(validation.spdxValid).toBe(true);
      expect(validation.cyclonedxValid).toBe(true);

      interface SpdxPackage {
        SPDXID: string;
        name: string;
        versionInfo?: string;
        externalRefs: { referenceCategory: string; referenceType: string; referenceLocator: string }[];
      }

      interface CycloneDxComponent {
        name: string;
        version: string;
        purl: string;
      }

      // 1. Inspect SPDX 2.3 output
      const spdxDoc = JSON.parse(readFileSync(join(tmp, 'tokentree-spdx-sbom.json'), 'utf8')) as {
        spdxVersion: string;
        dataLicense: string;
        packages: SpdxPackage[];
      };
      expect(spdxDoc.spdxVersion).toBe('SPDX-2.3');
      expect(spdxDoc.dataLicense).toBe('CC0-1.0');
      expect(spdxDoc.packages.length).toBeGreaterThan(50);

      const spdxRoot = spdxDoc.packages.find((p) => p.SPDXID === 'SPDXRef-Package-tokentree');
      expect(spdxRoot).toBeDefined();
      expect(spdxRoot?.versionInfo).toBe(releaseVersion);

      const spdxPackageNames = spdxDoc.packages.map((p) => p.name);
      // Assert representative Rust dependencies exist
      expect(spdxPackageNames).toContain('rusqlite');
      expect(spdxPackageNames).toContain('serde');
      expect(spdxPackageNames).toContain('clap');
      expect(spdxPackageNames).toContain('tokio');
      expect(spdxPackageNames).toContain('chrono');
      expect(spdxPackageNames).toContain('sha2');

      // Assert representative npm dependencies exist
      expect(spdxPackageNames).toContain('@tokentreehq/cli');
      expect(spdxPackageNames).toContain('@tokentreehq/database');

      // Check externalRefs PURLs in SPDX
      const rusqliteSpdx = spdxDoc.packages.find((p) => p.name === 'rusqlite');
      expect(rusqliteSpdx?.externalRefs[0].referenceLocator).toMatch(/^pkg:cargo\/rusqlite@/);

      // 2. Inspect CycloneDX 1.5 output
      const cdxDoc = JSON.parse(readFileSync(join(tmp, 'tokentree-cyclonedx-sbom.json'), 'utf8')) as {
        bomFormat: string;
        specVersion: string;
        $schema: string;
        metadata: { component: { version: string } };
        components: CycloneDxComponent[];
      };
      expect(cdxDoc.bomFormat).toBe('CycloneDX');
      expect(cdxDoc.specVersion).toBe('1.5');
      expect(cdxDoc.$schema).toBe('http://cyclonedx.org/schema/bom-1.5.schema.json');
      expect(cdxDoc.metadata.component.version).toBe(releaseVersion);
      expect(cdxDoc.components.length).toBeGreaterThan(50);

      const cdxComponentNames = cdxDoc.components.map((c) => c.name);
      // Assert representative Rust dependencies exist
      expect(cdxComponentNames).toContain('rusqlite');
      expect(cdxComponentNames).toContain('serde');
      expect(cdxComponentNames).toContain('clap');
      expect(cdxComponentNames).toContain('tokio');
      expect(cdxComponentNames).toContain('chrono');
      expect(cdxComponentNames).toContain('sha2');

      // Assert representative npm dependencies exist
      expect(cdxComponentNames).toContain('@tokentreehq/cli');
      expect(cdxComponentNames).toContain('@tokentreehq/database');

      const rusqliteCdx = cdxDoc.components.find((c) => c.name === 'rusqlite');
      expect(rusqliteCdx?.purl).toMatch(/^pkg:cargo\/rusqlite@/);
    } finally {
      rmSync(tmp, { recursive: true, force: true });
    }
  });
});
