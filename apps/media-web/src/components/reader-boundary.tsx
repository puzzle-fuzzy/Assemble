import { Component, type ReactNode } from "react";
import { RequestError } from "./common";

export class ReaderBoundary extends Component<
  { children: ReactNode; retry: () => void },
  { failed: boolean }
> {
  state = { failed: false };

  static getDerivedStateFromError() {
    return { failed: true };
  }

  render() {
    // 升级后旧页可能引用已删除的分块；保留导航，让用户明确刷新后恢复。
    return this.state.failed ? (
      <RequestError status={null} retry={this.props.retry} />
    ) : (
      this.props.children
    );
  }
}
