import { Activity, Binary, Bot, Box, CheckSquare, Cloud, Database, Dices, FolderKanban, GitBranch, History, Layers, NotebookText, Puzzle, Radar, ScrollText, Send, Server, SlidersHorizontal, Webhook, type LucideIcon } from 'lucide-react';
import type { Section } from '@/lib/sections';

/** Sidebar / tab icon of each section. */
export const SECTION_ICON: Record<Section, LucideIcon> = {
  http: Send,
  environments: Layers,
  utilities: Binary,
  fakedata: Dices,
  load: Activity,
  mocks: Server,
  inspector: Radar,
  webhooks: Webhook,
  logs: ScrollText,
  history: History,
  infra: Box,
  data: Database,
  tunnels: Cloud,
  notes: NotebookText,
  todos: CheckSquare,
  git: GitBranch,
  ai: Bot,
  projects: FolderKanban,
  settings: SlidersHorizontal,
  plugins: Puzzle,
};

export function SectionIcon({ section, size }: { section: Section; size?: number }) {
  const Icon = SECTION_ICON[section] ?? Binary;
  return <Icon size={size} />;
}
