import { toast } from '@/components/ui/toast';
import { errorMessage } from '@/lib/ipc';
import { protoApi, type ProtoKind } from '@/lib/api/proto';
import { useWorkspace } from '@/store/workspace';
import { KIND_LABEL } from './shared';

export { ProtoEditor } from './ProtoEditor';
export { WsEditor } from './WsEditor';
export { GrpcEditor } from './GrpcEditor';
export { MqttEditor } from './MqttEditor';
export { SioEditor } from './SioEditor';
export { GraphqlSchemaPanel, type GraphqlInsert } from './GraphqlSchemaPanel';
export { MessageLog } from './MessageLog';
export { closeProtoSession, useLive, liveKey } from './store';
export { KIND_LABEL, KIND_BADGE, KindBadge } from './shared';

/** Menu entries for "New …" (label, kind) in the order they should appear. */
export const PROTO_NEW_ITEMS: { kind: ProtoKind; label: string }[] = [
  { kind: 'ws', label: 'New WebSocket' },
  { kind: 'grpc', label: 'New gRPC' },
  { kind: 'mqtt', label: 'New MQTT' },
  { kind: 'socketio', label: 'New Socket.IO' },
];

/**
 * Creates `<folder>/new-<kind>.<ext>.yaml` with starter content and refreshes the tree. Returns
 * the new file's path (open it as a tab and render it with `ProtoEditor`).
 */
export async function newProtoRequest(kind: ProtoKind, folder: string, name = `New ${KIND_LABEL[kind]}`): Promise<string | undefined> {
  const p = useWorkspace.getState().project;
  if (!p) return undefined;
  try {
    const path = await protoApi.create(p.root, folder, kind, name);
    await useWorkspace.getState().refresh();
    return path;
  } catch (e) {
    toast.error(`Could not create the ${KIND_LABEL[kind]} file`, errorMessage(e));
    return undefined;
  }
}
