import { useEffect, useState } from 'react';
import { AlertTriangle, Plus, Send, X } from 'lucide-react';
import { Empty, Spinner } from '@/components/ui/misc';
import { useNav, useViewId } from '@/store/nav';
import { useWorkbench } from '@/store/workbench';
import { docForParam, useWorkspace } from '@/store/workspace';
import { RequestTree } from './RequestTree';
import { RequestEditor } from './RequestEditor';
import { FolderEditor } from './FolderEditor';
import { ProtoEditor } from '@/features/proto';

/**
 * One `http` view: the shared request tree plus the editor of this view's document (`param` = the
 * request, WebSocket/gRPC/MQTT/Socket.IO file, or `<folder>/_folder.yaml`). Requests open as
 * workbench tabs, so several views can show different requests side by side.
 */
export default function HttpPage() {
  const param = useNav((s) => s.param);
  return (
    <div className="split">
      <RequestTree />
      <div className="pane fill">{param ? <HttpDocView key={param} param={param} /> : <Landing />}</div>
    </div>
  );
}

function HttpDocView({ param }: { param: string }) {
  const project = useWorkspace((s) => s.project!);
  const doc = useWorkspace((s) => docForParam(s.tabs, param));
  const viewId = useViewId();
  const [failed, setFailed] = useState(false);

  useEffect(() => {
    let alive = true;
    setFailed(false);
    void useWorkspace
      .getState()
      .ensureDoc(param)
      .then((t) => alive && !t && setFailed(true));
    return () => {
      alive = false;
    };
  }, [param, project.root]);

  if (doc?.kind === 'request') return <RequestEditor key={doc.id} tab={doc} />;
  if (doc?.kind === 'folder') return <FolderEditor key={doc.id} tab={doc} />;
  if (doc?.kind === 'proto') return <ProtoEditor key={doc.id} root={project.root} path={doc.path} protocol={doc.protocol} />;
  if (failed)
    return (
      <Empty
        icon={<AlertTriangle />}
        title="Could not open this file"
        action={
          viewId ? (
            <button className="btn btn-sm" onClick={() => void useWorkbench.getState().close(viewId)}>
              <X /> Close tab
            </button>
          ) : undefined
        }
      >
        <code className="code-inline">http/{param}</code> is missing or cannot be parsed.
      </Empty>
    );
  return (
    <div className="empty">
      <Spinner />
    </div>
  );
}

function Landing() {
  const newRequest = useWorkspace((s) => s.newRequest);
  return (
    <Empty
      icon={<Send />}
      title="Open or create a request"
      action={
        <button className="btn btn-primary" onClick={() => newRequest('')}>
          <Plus /> New request
        </button>
      }
    >
      Requests are plain YAML files under <code className="code-inline">http/</code> in your project — commit them with git. Click one in the tree to preview it, double-click to keep it open, and
      drag tabs to the side to compare requests. Tip: paste a cURL command straight into the URL bar.
    </Empty>
  );
}
