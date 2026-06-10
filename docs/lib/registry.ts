/**
 * The single source of truth for which docs pages exist and how they are
 * grouped in the sidebar. Every slug listed here must have a matching
 * content file registered in `pages` below — the build fails otherwise.
 */

import type { DocPage, NavGroup } from './types';

import { page as introduction } from '@/content/introduction';
import { page as installation } from '@/content/installation';
import { page as quickstart } from '@/content/quickstart';

import { page as agentsOverview } from '@/content/agents-overview';
import { page as llmAgent } from '@/content/llm-agent';
import { page as workflowAgents } from '@/content/workflow-agents';
import { page as multiAgent } from '@/content/multi-agent';
import { page as callbacksAndPlugins } from '@/content/callbacks-and-plugins';

import { page as runner } from '@/content/runner';
import { page as events } from '@/content/events';
import { page as streaming } from '@/content/streaming';
import { page as sessionsAndState } from '@/content/sessions-and-state';
import { page as artifacts } from '@/content/artifacts';
import { page as memory } from '@/content/memory';

import { page as models } from '@/content/models';
import { page as providers } from '@/content/providers';

import { page as toolsOverview } from '@/content/tools-overview';
import { page as functionTools } from '@/content/function-tools';
import { page as builtinTools } from '@/content/builtin-tools';
import { page as openapiTools } from '@/content/openapi-tools';
import { page as mcp } from '@/content/mcp';

import { page as structuredOutput } from '@/content/structured-output';
import { page as toolConfirmation } from '@/content/tool-confirmation';
import { page as cancellationAndResume } from '@/content/cancellation-and-resume';
import { page as contextCaching } from '@/content/context-caching';
import { page as eventCompaction } from '@/content/event-compaction';
import { page as codeExecution } from '@/content/code-execution';
import { page as auth } from '@/content/auth';

import { page as server } from '@/content/server';
import { page as cli } from '@/content/cli';
import { page as a2a } from '@/content/a2a';
import { page as telemetry } from '@/content/telemetry';
import { page as evalPage } from '@/content/eval';
import { page as testing } from '@/content/testing';
import { page as security } from '@/content/security';
import { page as errors } from '@/content/errors';

import { page as exampleGeminiChat } from '@/content/examples-gemini-chat';
import { page as exampleWeatherAgent } from '@/content/examples-weather-agent';
import { page as exampleThreeProviders } from '@/content/examples-three-providers';
import { page as exampleCodeAgent } from '@/content/examples-code-agent';

import { page as guideMultiAgentPipeline } from '@/content/guides-multi-agent-pipeline';
import { page as guidePersistentSessions } from '@/content/guides-persistent-sessions';
import { page as guideProductionDeploy } from '@/content/guides-production-deploy';

const all: DocPage[] = [
  introduction,
  installation,
  quickstart,
  agentsOverview,
  llmAgent,
  workflowAgents,
  multiAgent,
  callbacksAndPlugins,
  runner,
  events,
  streaming,
  sessionsAndState,
  artifacts,
  memory,
  models,
  providers,
  toolsOverview,
  functionTools,
  builtinTools,
  openapiTools,
  mcp,
  structuredOutput,
  toolConfirmation,
  cancellationAndResume,
  contextCaching,
  eventCompaction,
  codeExecution,
  auth,
  server,
  cli,
  a2a,
  telemetry,
  evalPage,
  testing,
  security,
  errors,
  exampleGeminiChat,
  exampleWeatherAgent,
  exampleThreeProviders,
  exampleCodeAgent,
  guideMultiAgentPipeline,
  guidePersistentSessions,
  guideProductionDeploy,
];

export const nav: NavGroup[] = [
  {
    label: 'Start',
    slugs: ['introduction', 'installation', 'quickstart'],
  },
  {
    label: 'Agents',
    slugs: ['agents-overview', 'llm-agent', 'workflow-agents', 'multi-agent', 'callbacks-and-plugins'],
  },
  {
    label: 'Runtime',
    slugs: ['runner', 'events', 'streaming', 'sessions-and-state', 'artifacts', 'memory'],
  },
  {
    label: 'Models',
    slugs: ['models', 'providers'],
  },
  {
    label: 'Tools',
    slugs: ['tools-overview', 'function-tools', 'builtin-tools', 'openapi-tools', 'mcp'],
  },
  {
    label: 'Advanced',
    slugs: [
      'structured-output',
      'tool-confirmation',
      'cancellation-and-resume',
      'context-caching',
      'event-compaction',
      'code-execution',
      'auth',
    ],
  },
  {
    label: 'Operate',
    slugs: ['server', 'cli', 'a2a', 'telemetry', 'eval', 'testing', 'security', 'errors'],
  },
  {
    label: 'Examples',
    slugs: [
      'examples/gemini-chat',
      'examples/weather-agent',
      'examples/three-providers',
      'examples/code-agent',
    ],
  },
  {
    label: 'Guides',
    slugs: [
      'guides/multi-agent-pipeline',
      'guides/persistent-sessions',
      'guides/production-deploy',
    ],
  },
];

const bySlug = new Map<string, DocPage>(all.map((p) => [p.slug, p]));

export const orderedSlugs: string[] = nav.flatMap((g) => g.slugs);

export function getPage(slug: string): DocPage | undefined {
  return bySlug.get(slug);
}

export function groupOf(slug: string): NavGroup | undefined {
  return nav.find((g) => g.slugs.includes(slug));
}

export function prevNext(slug: string): { prev?: DocPage; next?: DocPage } {
  const i = orderedSlugs.indexOf(slug);
  return {
    prev: i > 0 ? bySlug.get(orderedSlugs[i - 1]) : undefined,
    next: i >= 0 && i < orderedSlugs.length - 1 ? bySlug.get(orderedSlugs[i + 1]) : undefined,
  };
}

export const REPO_URL = 'https://github.com/skundu42/adk-rs';
export const CRATE_VERSION = '0.3.0';
