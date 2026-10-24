import { useEffect, useRef, useState } from 'react';
import { Check, Dices, Shuffle } from 'lucide-react';
import { CodeEditor } from '@/components/editor/CodeEditor';
import { Modal } from '@/components/ui/Modal';
import { Callout, CopyButton, Spinner, Switch } from '@/components/ui/misc';
import { errorMessage } from '@/lib/ipc';
import { dataApi } from '@/lib/api/data';
import './data.css';

export interface RandomizeDialogProps {
  open: boolean;
  onClose(): void;
  /** The real JSON response body to randomize (editable in the dialog). */
  body: string;
  title?: string;
  /** When set, shows a primary button that hands the template back (e.g. to save it as a stub's
   * body with `transformers: ["response-template"]`). */
  onApply?(template: string): void;
  applyLabel?: string;
  /** Initial "keep ids stable" setting. */
  keepIds?: boolean;
}

/**
 * Request → mock → randomize: turns a JSON body into a WireMock `response-template` body with the
 * same structure whose values are random-data helpers (`{{random 'Name.firstName'}}`,
 * `{{randomInt lower=1 upper=100}}`, …), and shows a locally rendered sample.
 * Reusable by the mock and HTTP modules.
 */
export function RandomizeDialog({ open, onClose, body, title = 'Randomize response', onApply, applyLabel = 'Use template', keepIds: keepIdsInitial = false }: RandomizeDialogProps) {
  const [input, setInput] = useState(body);
  const [keepIds, setKeepIds] = useState(keepIdsInitial);
  const [template, setTemplate] = useState('');
  const [sample, setSample] = useState('');
  const [seed, setSeed] = useState(1);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const seq = useRef(0);

  useEffect(() => {
    if (open) setInput(body);
  }, [open, body]);

  useEffect(() => {
    if (!open) return;
    const n = ++seq.current;
    if (!input.trim()) {
      setTemplate('');
      setSample('');
      setError(null);
      setBusy(false);
      return;
    }
    setBusy(true);
    const t = setTimeout(async () => {
      try {
        const tpl = await dataApi.toTemplate(input, keepIds);
        const s = await dataApi.templateSample(tpl, seed);
        if (n !== seq.current) return;
        setTemplate(tpl);
        setSample(s);
        setError(null);
      } catch (e) {
        if (n === seq.current) setError(errorMessage(e));
      } finally {
        if (n === seq.current) setBusy(false);
      }
    }, 200);
    return () => clearTimeout(t);
  }, [open, input, keepIds, seed]);

  return (
    <Modal
      open={open}
      onClose={onClose}
      title={title}
      size="xl"
      headerExtra={busy ? <Spinner /> : undefined}
      footer={
        <>
          <span className="subtle xs grow">
            Helpers: randomValue, randomInt, randomDecimal, now, pickRandom and random '&lt;Category.key&gt;' — rendered by any WireMock-compatible engine with response templating on.
          </span>
          <button className="btn" onClick={onClose}>
            Close
          </button>
          {onApply && (
            <button
              className="btn btn-primary"
              disabled={!template || !!error}
              onClick={() => {
                onApply(template);
                onClose();
              }}
            >
              <Check /> {applyLabel}
            </button>
          )}
        </>
      }
    >
      <div className="data-randomize">
        <div className="col-sm">
          <div className="row">
            <span className="label grow">Original body</span>
            <Switch checked={keepIds} onChange={setKeepIds} label={<span className="xs">Keep ids stable</span>} title="Keep id, …Id and …_id values so /users/42 stays user 42" />
          </div>
          <div className="editor-shell fill">
            <CodeEditor value={input} onChange={setInput} language="json" path="randomize-input.json" minimal />
          </div>
        </div>
        <div className="col-sm">
          <div className="row">
            <span className="label grow">Response template</span>
            <CopyButton text={template} label="Copy" />
          </div>
          {error ? (
            <Callout kind="danger">{error}</Callout>
          ) : (
            <div className="editor-shell fill">
              <CodeEditor value={template} language="handlebars" readOnly minimal />
            </div>
          )}
        </div>
        <div className="col-sm">
          <div className="row">
            <span className="label grow">Sample</span>
            <button className="btn btn-sm btn-ghost" onClick={() => setSeed((s) => s + 1)} title="Render another sample" disabled={!template}>
              <Shuffle /> Another
            </button>
            <CopyButton text={sample} />
          </div>
          {template ? (
            <div className="editor-shell fill">
              <CodeEditor value={sample} language="json" readOnly minimal />
            </div>
          ) : (
            <div className="empty">
              <Dices />
              <p>Paste a JSON body on the left.</p>
            </div>
          )}
        </div>
      </div>
    </Modal>
  );
}

export default RandomizeDialog;
