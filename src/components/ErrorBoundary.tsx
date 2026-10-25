import { Component, type ErrorInfo, type ReactNode } from "react";
import { withTranslation, type WithTranslation } from "react-i18next";
import { ErrorReportHost } from "../features/errorReport/ErrorReportDialog";
import { openErrorDetails, reportUnexpected, sourceFromFrontend } from "../features/errorReport/errorReport";
import type { ErrorReportSource } from "../state/slices/ui";
import { Button } from "./Button";

/** Where a render crash is reported from (the report's "where" field; an identifier, not UI copy). */
const BOUNDARY_CONTEXT = "ErrorBoundary";
const LINE_BREAK = "\n";

interface Props extends WithTranslation {
  children: ReactNode;
  /**
   * The app-wide boundary: its children include the dialog host, which a crash unmounts, so the error details dialog
   * is rendered next to the Retry panel.
   */
  reportHost?: boolean;
}
interface State {
  failed: boolean;
  source: ErrorReportSource | null;
}

/** Contains a failing subtree (e.g. the lazily loaded terminal library) so the rest of the shell keeps working. */
class Boundary extends Component<Props, State> {
  state: State = { failed: false, source: null };

  static getDerivedStateFromError(): Partial<State> {
    return { failed: true };
  }

  componentDidCatch(error: Error, info: ErrorInfo): void {
    console.error(error, info.componentStack);
    // A render crash is unexpected by definition: the error details dialog opens by itself (§8.14).
    const stack = [error.stack, info.componentStack].filter((s) => s).join(LINE_BREAK);
    const source = sourceFromFrontend(error.message, BOUNDARY_CONTEXT, stack);
    this.setState({ source });
    // The app-wide boundary unmounted the dialogs and the notices with everything else: a stale open dialog must not
    // keep the report from showing, so it is replaced.
    reportUnexpected(source, this.props.reportHost === true);
  }

  render() {
    const { t, children, reportHost } = this.props;
    if (!this.state.failed) return children;
    return (
      <>
        <div className="error-boundary" role="alert">
          <p>{t("app.viewFailed")}</p>
          <div className="error-boundary__actions">
            <Button onClick={() => this.setState({ failed: false, source: null })}>{t("app.retry")}</Button>
            <Button onClick={() => openErrorDetails(this.state.source)}>{t("errorReport.open")}</Button>
          </div>
        </div>
        {reportHost ? <ErrorReportHost /> : null}
      </>
    );
  }
}

export const ErrorBoundary = withTranslation()(Boundary);
