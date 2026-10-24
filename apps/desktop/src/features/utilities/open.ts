import { create } from 'zustand';
import type { UtilArgs } from '@/lib/api/utils';
import { useNav } from '@/store/nav';

interface UtilityLink {
  op: string;
  input: string;
  args?: UtilArgs;
  nonce: number;
}

/** A pending "open this in Utilities" request (bodies can be large, so not a nav param). */
export const useUtilityLink = create<{ link?: UtilityLink }>(() => ({}));

/** Opens the Utilities page on `op` with `input` (and optional args, e.g. a jq filter). */
export function openInUtilities(op: string, input: string, args?: UtilArgs) {
  useUtilityLink.setState({ link: { op, input, args, nonce: Date.now() } });
  useNav.getState().go('utilities');
}
