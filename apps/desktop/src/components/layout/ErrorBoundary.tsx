import { Component, type ReactNode } from 'react';
import { AlertTriangle } from 'lucide-react';

export class ErrorBoundary extends Component<{ children: ReactNode }, { error?: Error }> {
  state: { error?: Error } = {};
  static getDerivedStateFromError(error: Error) {
    return { error };
  }
  render() {
    if (!this.state.error) return this.props.children;
    return (
      <div className="empty">
        <AlertTriangle />
        <h3>Something went wrong on this page</h3>
        <pre className="code" style={{ maxWidth: 720, textAlign: 'left' }}>
          {this.state.error.message}
        </pre>
        <button className="btn" onClick={() => this.setState({ error: undefined })}>
          Try again
        </button>
      </div>
    );
  }
}
