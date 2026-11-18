import { Component, type ErrorInfo, type ReactNode } from "react";
import { CrashScreen } from "./CrashScreen";
import { crashDetailsFrom, type CrashDetails } from "./crash";

type State = { details: CrashDetails | null };

/**
 * Catches an error thrown while drawing the app and offers the same report as
 * a native crash. Reloading starts the interface again (D31).
 */
export class CrashBoundary extends Component<{ children: ReactNode }, State> {
  state: State = { details: null };

  static getDerivedStateFromError(error: unknown): State {
    return { details: crashDetailsFrom(error) };
  }

  componentDidCatch(_error: unknown, info: ErrorInfo) {
    const componentStack = info.componentStack?.trim();
    if (!componentStack) return;
    this.setState(({ details }) =>
      details
        ? {
            details: {
              ...details,
              backtrace: [details.backtrace, `Component stack:\n${componentStack}`]
                .filter(Boolean)
                .join("\n\n"),
            },
          }
        : null,
    );
  }

  render() {
    const { details } = this.state;
    if (details) {
      return (
        <CrashScreen
          details={details}
          onReload={() => window.location.reload()}
        />
      );
    }
    return this.props.children;
  }
}
