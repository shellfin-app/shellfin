import { Component, type ErrorInfo, type ReactNode } from "react";
import { withTranslation, type WithTranslation } from "react-i18next";
import { Button } from "./Button";

interface Props extends WithTranslation {
  children: ReactNode;
}
interface State {
  failed: boolean;
}

/** Contains a failing subtree (e.g. the lazily loaded terminal library) so the rest of the shell keeps working. */
class Boundary extends Component<Props, State> {
  state: State = { failed: false };

  static getDerivedStateFromError(): State {
    return { failed: true };
  }

  componentDidCatch(error: Error, info: ErrorInfo): void {
    console.error(error, info.componentStack);
  }

  render() {
    const { t, children } = this.props;
    if (!this.state.failed) return children;
    return (
      <div className="error-boundary" role="alert">
        <p>{t("app.viewFailed")}</p>
        <Button onClick={() => this.setState({ failed: false })}>{t("app.retry")}</Button>
      </div>
    );
  }
}

export const ErrorBoundary = withTranslation()(Boundary);
