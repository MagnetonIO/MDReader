import { readFileSync } from 'node:fs';

import { parse } from 'yaml';

import { validateMobileBetaWorkflow } from './mobile-beta-workflow-contract';

const path = process.argv[2];
if (!path) {
  throw new Error('usage: check-mobile-beta-workflow.ts <workflow.yml>');
}
validateMobileBetaWorkflow(parse(readFileSync(path, 'utf8')));
process.stdout.write(`mobile beta workflow contract valid: ${path}\n`);
