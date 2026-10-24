import { useEffect } from 'react';
import { Empty } from '@/components/ui/misc';
import { protoKindOf, type ProtoKind } from '@/lib/api/proto';
import { useKeepView } from '@/store/nav';
import { liveKey, useLive } from './store';
import { GrpcEditor } from './GrpcEditor';
import { MqttEditor } from './MqttEditor';
import { SioEditor } from './SioEditor';
import { WsEditor } from './WsEditor';
import './proto.css';

/**
 * Editor for a non-HTTP request file under `http/`. `protocol` is the tree node's protocol
 * (`ws`, `grpc`, `mqtt`, `socketio`); when omitted it's derived from the file extension.
 */
export function ProtoEditor({ root, path, protocol }: { root: string; path: string; protocol?: ProtoKind | string }) {
  const kind = (protocol && protocol !== 'http' ? protocol : protoKindOf(path)) as ProtoKind | null;
  // A live connection keeps its tab: replacing a preview tab would close the connection.
  const live = useLive((s) => s.live[liveKey(root, path)]?.status);
  const keepView = useKeepView();
  useEffect(() => {
    if (live === 'connecting' || live === 'open') keepView();
  }, [live, keepView]);
  switch (kind) {
    case 'ws':
      return <WsEditor key={path} root={root} path={path} />;
    case 'grpc':
      return <GrpcEditor key={path} root={root} path={path} />;
    case 'mqtt':
      return <MqttEditor key={path} root={root} path={path} />;
    case 'socketio':
      return <SioEditor key={path} root={root} path={path} />;
    default:
      return <Empty title="Unsupported file">{path} is not a WebSocket, gRPC, MQTT or Socket.IO file.</Empty>;
  }
}
