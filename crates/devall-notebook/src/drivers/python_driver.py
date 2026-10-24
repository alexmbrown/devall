# /dev/all notebook kernel for Python 3 (started by the app; not meant to be run by hand).
#
# Protocol: the host writes JSON lines on stdin ({"t":"exec","id","code","cwd","env"} and
# {"t":"interrupt"}); the kernel answers with lines on stdout that start with "\x1eDEVALL" followed
# by JSON ({"t":"ready"|"stream"|"display"|"error"|"done", "id", ...}). Anything else on stdout or
# stderr (subprocesses, C extensions) is the running cell's raw output.
#
# Cells share one globals dict. The last expression's value is displayed like Jupyter (a trailing
# ";" hides it), objects with _repr_html_/_repr_png_/... render richly, matplotlib figures are
# shown as PNG after the cell, and top-level `await` works.

import ast
import asyncio
import base64
import builtins
import inspect
import io
import json
import linecache
import os
import queue
import signal
import sys
import threading
import traceback
import warnings

MARK = "\x1eDEVALL"
DRIVER = os.path.abspath(__file__)
ASYNCIO_DIR = os.path.dirname(os.path.abspath(asyncio.__file__))

_lock = threading.Lock()
_current = [0]
_counter = [0]
_running = threading.Event()
_queue = queue.Queue()
_proto_fd = 1
_input = None


def _detach_stdin():
    """Reads the protocol from a private (non-inheritable) copy of stdin and gives fd 0 the null
    device. Child processes then never inherit the protocol pipe — on Windows a child inheriting a
    pipe that another thread is blocked reading hangs process creation."""
    global _input
    fd = os.dup(0)
    null = os.open(os.devnull, os.O_RDONLY)
    os.dup2(null, 0)
    os.close(null)
    _input = io.open(fd, "rb", buffering=65536)


def _send(msg):
    if "id" not in msg:
        msg["id"] = _current[0]
    try:
        text = json.dumps(msg, ensure_ascii=True)
    except (TypeError, ValueError):
        text = json.dumps(msg, ensure_ascii=True, default=repr)
    data = (MARK + text + "\n").encode("ascii")
    with _lock:
        view = memoryview(data)
        while view:
            n = os.write(_proto_fd, view)
            view = view[n:]


class _Bytes(object):
    """`sys.stdout.buffer` stand-in: bytes are decoded and sent as text."""

    def __init__(self, stream):
        self._stream = stream

    def write(self, b):
        self._stream.write(bytes(b).decode("utf-8", "replace"))
        return len(b)

    def flush(self):
        pass


class _Stream(io.TextIOBase):
    def __init__(self, name, fd):
        super().__init__()
        self._name = name
        self._fd = fd
        self.buffer = _Bytes(self)

    @property
    def name(self):
        return "<%s>" % self._name

    @property
    def encoding(self):
        return "utf-8"

    @property
    def errors(self):
        return "replace"

    def writable(self):
        return True

    def isatty(self):
        return False

    def fileno(self):
        return self._fd

    def write(self, s):
        if not isinstance(s, str):
            raise TypeError("write() argument must be str, not %s" % type(s).__name__)
        if s:
            _send({"t": "stream", "name": self._name, "text": s})
        return len(s)

    def writelines(self, lines):
        for line in lines:
            self.write(line)

    def flush(self):
        pass


def _no_input(prompt=""):
    raise RuntimeError("input() isn't available in /dev/all notebook cells - set values in the code or use environment variables (DEVALL_*).")


# ---------- rich display ----------

_REPRS = [
    ("_repr_html_", "text/html"),
    ("_repr_markdown_", "text/markdown"),
    ("_repr_svg_", "image/svg+xml"),
    ("_repr_png_", "image/png"),
    ("_repr_jpeg_", "image/jpeg"),
    ("_repr_json_", "application/json"),
    ("_repr_latex_", "text/latex"),
]
_shown_figures = set()


def _figure_png(fig):
    buf = io.BytesIO()
    fig.savefig(buf, format="png", bbox_inches="tight")
    return base64.b64encode(buf.getvalue()).decode("ascii")


def _bundle(value):
    data = {}
    try:
        data["text/plain"] = repr(value)
    except Exception as e:  # noqa: BLE001
        data["text/plain"] = "<repr failed: %r>" % (e,)
    if isinstance(value, type):
        return data
    for meth, mime in _REPRS:
        f = getattr(value, meth, None)
        if f is None or not callable(f):
            continue
        try:
            r = f()
        except Exception:  # noqa: BLE001
            continue
        if isinstance(r, tuple) and r:
            r = r[0]
        if r is None:
            continue
        if isinstance(r, (bytes, bytearray)):
            r = base64.b64encode(bytes(r)).decode("ascii")
        data[mime] = r
    mfig = sys.modules.get("matplotlib.figure")
    if mfig is not None and isinstance(value, getattr(mfig, "Figure", ())):
        try:
            data["image/png"] = _figure_png(value)
            _shown_figures.add(id(value))
        except Exception:  # noqa: BLE001
            pass
    return data


def display(*objs, **kwargs):
    """Shows objects below the cell (like IPython's display)."""
    raw = kwargs.get("raw", False)
    for obj in objs:
        if raw and isinstance(obj, dict):
            _send({"t": "display", "data": obj, "result": False})
        else:
            _send({"t": "display", "data": _bundle(obj), "result": False})


class HTML(object):
    def __init__(self, data):
        self.data = data

    def _repr_html_(self):
        return self.data


class Markdown(object):
    def __init__(self, data):
        self.data = data

    def _repr_markdown_(self):
        return self.data


def _flush_figures():
    plt = sys.modules.get("matplotlib.pyplot")
    if plt is None:
        return
    try:
        for num in plt.get_fignums():
            fig = plt.figure(num)
            if id(fig) not in _shown_figures:
                _send({"t": "display", "data": {"image/png": _figure_png(fig), "text/plain": repr(fig)}, "result": False})
        plt.close("all")
    except Exception:  # noqa: BLE001
        pass
    _shown_figures.clear()


# ---------- execution ----------

_ns = {"__name__": "__main__", "__builtins__": builtins, "display": display, "HTML": HTML, "Markdown": Markdown}
_loop = asyncio.new_event_loop()
asyncio.set_event_loop(_loop)
_TLA = getattr(ast, "PyCF_ALLOW_TOP_LEVEL_AWAIT", 0)


def _run_code(code_obj):
    result = eval(code_obj, _ns)
    if code_obj.co_flags & inspect.CO_COROUTINE:
        result = _loop.run_until_complete(result)
    return result


def _run_cell(code):
    _counter[0] += 1
    fname = "<cell-%d>" % _counter[0]
    linecache.cache[fname] = (len(code), None, code.splitlines(True), fname)
    tree = compile(code, fname, "exec", flags=ast.PyCF_ONLY_AST | _TLA, dont_inherit=True)
    last = None
    if tree.body and isinstance(tree.body[-1], ast.Expr) and not code.rstrip().endswith(";"):
        last = ast.Expression(tree.body.pop().value)
    if tree.body:
        _run_code(compile(tree, fname, "exec", flags=_TLA, dont_inherit=True))
    if last is not None:
        value = _run_code(compile(last, fname, "eval", flags=_TLA, dont_inherit=True))
        if value is not None:
            _ns["_"] = value
            _send({"t": "display", "data": _bundle(value), "result": True})


def _keep_frame(frame):
    f = os.path.abspath(frame.filename) if not frame.filename.startswith("<") else frame.filename
    return f != DRIVER and not f.startswith(ASYNCIO_DIR)


def _filter(te):
    seen = set()
    while te is not None and id(te) not in seen:
        seen.add(id(te))
        te.stack = traceback.StackSummary.from_list([f for f in te.stack if _keep_frame(f)])
        if te.__cause__ is not None:
            _filter(te.__cause__)
        te = te.__context__


def _send_exception(e):
    te = traceback.TracebackException(type(e), e, e.__traceback__)
    _filter(te)
    text = "".join(te.format())
    _send({"t": "error", "ename": type(e).__name__, "evalue": str(e), "traceback": text.rstrip("\n").split("\n")})


def _exec(msg):
    _current[0] = msg.get("id", 0)
    env = msg.get("env") or {}
    saved = {k: os.environ.get(k) for k in env}
    ok, interrupted = True, False
    try:
        try:
            os.environ.update(env)
            cwd = msg.get("cwd")
            if cwd:
                try:
                    os.chdir(cwd)
                except OSError as e:
                    _send({"t": "stream", "name": "stderr", "text": "warning: can't change to %s: %s\n" % (cwd, e)})
            _running.set()
            try:
                _run_cell(msg.get("code", ""))
            finally:
                _running.clear()
        except KeyboardInterrupt:
            ok, interrupted = False, True
            _send({"t": "error", "ename": "KeyboardInterrupt", "evalue": "", "traceback": ["KeyboardInterrupt"]})
        except SystemExit as e:
            if e.code not in (None, 0):
                ok = False
                _send({"t": "error", "ename": "SystemExit", "evalue": str(e.code), "traceback": ["SystemExit: %s" % (e.code,)]})
        except BaseException as e:  # noqa: BLE001
            ok = False
            _send_exception(e)
        finally:
            for k, v in saved.items():
                if v is None:
                    os.environ.pop(k, None)
                else:
                    os.environ[k] = v
            _flush_figures()
            sys.stdout.flush()
    except KeyboardInterrupt:
        pass  # an interrupt that arrived as the cell finished
    while True:
        try:
            _send({"t": "done", "ok": ok, "interrupted": interrupted})
            break
        except KeyboardInterrupt:
            continue


def _interrupt_main():
    try:
        if hasattr(signal, "pthread_kill"):
            signal.pthread_kill(threading.main_thread().ident, signal.SIGINT)
        else:
            # Windows: runs the C handler in this thread, which also wakes time.sleep() in main.
            signal.raise_signal(signal.SIGINT)
    except Exception:  # noqa: BLE001
        import _thread

        _thread.interrupt_main()


def _reader():
    try:
        for raw in _input:
            try:
                msg = json.loads(raw.decode("utf-8"))
            except ValueError:
                continue
            if msg.get("t") == "interrupt":
                if _running.is_set():
                    _interrupt_main()
            else:
                _queue.put(msg)
    finally:
        # The app went away: don't linger.
        os._exit(0)


def main():
    signal.signal(signal.SIGINT, signal.default_int_handler)
    _detach_stdin()
    sys.path[0] = ""
    sys.stdout = _Stream("stdout", 1)
    sys.stderr = _Stream("stderr", 2)
    sys.stdin = io.StringIO("")
    builtins.input = _no_input
    builtins.display = display
    warnings.filterwarnings("ignore", message=".*non-interactive.*cannot be shown")
    warnings.filterwarnings("ignore", message=".*FigureCanvasAgg is non-interactive.*")
    threading.Thread(target=_reader, name="devall-stdin", daemon=True).start()
    _send({"t": "ready", "id": 0, "version": sys.version.split()[0]})
    while True:
        try:
            msg = _queue.get(timeout=0.25)
        except queue.Empty:
            continue
        except KeyboardInterrupt:
            continue
        try:
            if msg.get("t") == "exec":
                _exec(msg)
        except KeyboardInterrupt:
            continue


if __name__ == "__main__":
    main()
