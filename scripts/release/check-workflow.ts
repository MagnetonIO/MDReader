import { readFileSync } from 'node:fs';

import { parse } from 'yaml';

import { validateReleaseWorkflow } from './workflow-contract';

const path = process.argv[2];
if (!path) throw new Error('usage: check-workflow.ts <workflow.yml>');
validateReleaseWorkflow(parse(readFileSync(path, 'utf8')));
process.stdout.write(`release workflow contract valid: ${path}\n`);
