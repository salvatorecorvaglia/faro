import { Component, type ReactNode } from 'react';
import { EmptyState } from './ui';

/** Keep the shell usable if a deferred tab module fails to load or render. */
export class LoadBoundary extends Component<{ children: ReactNode }, { failed: boolean }> {
  override state = { failed: false };
  static getDerivedStateFromError() {
    return { failed: true };
  }
  override render() {
    return this.state.failed ? (
      <EmptyState title="This tab could not load" hint="Restart Faro to try again." />
    ) : (
      this.props.children
    );
  }
}
