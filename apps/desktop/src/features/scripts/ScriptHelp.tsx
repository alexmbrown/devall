import { HELP, LIMITS, UNSUPPORTED } from './apiReference';

/** The supported script API, grouped. Rendered next to the script editors. */
export function ScriptHelp() {
  return (
    <div className="script-help">
      <p className="subtle xs">
        Scripts are Postman-compatible JavaScript (QuickJS sandbox). Folder scripts run first, outermost folder first, then the request’s own script; each sees the variables
        and request edits made before it.
      </p>
      {HELP.map((section) => (
        <section key={section.title}>
          <h4>{section.title}</h4>
          <dl>
            {section.items.map((it) => (
              <div key={it.code} className="script-help-item">
                <dt>
                  <code>{it.code}</code>
                </dt>
                <dd>{it.doc}</dd>
              </div>
            ))}
          </dl>
        </section>
      ))}
      <section>
        <h4>Not supported</h4>
        <ul>
          {UNSUPPORTED.map((u) => (
            <li key={u}>{u}</li>
          ))}
        </ul>
      </section>
      <p className="subtle xs">Limits: {LIMITS}.</p>
    </div>
  );
}
