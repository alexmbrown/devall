import { DriftPanel } from '@/features/openapi';
import type { DriftFinding } from '@/lib/api/openapi';
import { toast } from '@/components/ui/toast';
import { useMappings } from '@/features/mocks/lib/queries';
import { stubId } from '@/features/mocks/lib/stub-utils';
import { openStub } from '@/features/mocks/stubs/open';
import { openStubFile } from '@/features/mocks/stubs/openFile';

/**
 * Spec ↔ mock ↔ traffic drift: do the stubs in mocks/mappings (and the traffic the inspector
 * captured) agree with the project's OpenAPI spec? Works without a running server.
 */
export default function DriftPage() {
  const mappings = useMappings();
  const open = (f: DriftFinding) => {
    const m = mappings.data?.find((x) => stubId(x) === f.refId);
    if (m) return openStub(m);
    if (f.source.startsWith('mocks/')) return openStubFile(f.source.slice('mocks/'.length));
    toast.info('Stub not loaded', f.source);
  };
  return (
    <div className="page-body">
      <DriftPanel initial={{ mocks: true }} onOpenStub={open} />
    </div>
  );
}
