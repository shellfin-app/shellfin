import {
  cloneElement,
  isValidElement,
  useId,
  useState,
  type InputHTMLAttributes,
  type ReactElement,
  type ReactNode,
  type SelectHTMLAttributes,
} from "react";
import { useTranslation } from "react-i18next";
import { Icon } from "./Icon";
import { InfoTip } from "./InfoTip";
import "./field.css";

/**
 * One settings row: label (plus an optional (?) tip) on the left, the control on the right. Unlike `Field`, the label
 * is linked with `htmlFor`, so the tip button sits next to the label text without becoming part of it. `hint` is one
 * short line below; the longer explanation goes into `info`.
 */
export function Setting({
  label,
  info,
  hint,
  error,
  children,
}: {
  label: string;
  info?: string;
  hint?: string;
  error?: string | null;
  children: ReactElement<{ id?: string }>;
}) {
  const autoId = useId();
  const controlId = isValidElement<{ id?: string }>(children) ? (children.props.id ?? autoId) : autoId;
  return (
    <div className="setting">
      <div className="setting__row">
        <span className="setting__label">
          <label htmlFor={controlId}>{label}</label>
          {info ? <InfoTip setting={label} text={info} /> : null}
        </span>
        <div className="setting__control">{cloneElement(children, { id: controlId })}</div>
      </div>
      {hint ? <span className="field__hint">{hint}</span> : null}
      {error ? (
        <span className="dialog__error" role="alert">
          {error}
        </span>
      ) : null}
    </div>
  );
}

/**
 * Label wrapping exactly one control (the label text is the control's accessible name). `hint` and `error` are
 * rendered OUTSIDE the label so they do not become part of the control's name.
 */
export function Field({
  label,
  children,
  hint,
  error,
}: {
  label: string;
  children: ReactNode;
  hint?: string;
  error?: string | null;
}) {
  return (
    <div className="field">
      <label className="field__control">
        <span className="field__label">{label}</span>
        {children}
      </label>
      {hint ? <span className="field__hint">{hint}</span> : null}
      {error ? (
        <span className="dialog__error" role="alert">
          {error}
        </span>
      ) : null}
    </div>
  );
}

/** Label for a compound control (several focusable parts). */
export function FieldGroup({
  label,
  children,
  error,
  info,
}: {
  label: string;
  children: ReactNode;
  error?: string | null;
  /** Longer explanation, shown by a (?) tip next to the label. */
  info?: string;
}) {
  const id = useId();
  return (
    <div className="field" role="group" aria-labelledby={id}>
      <span className="setting__label">
        <span id={id} className="field__label">
          {label}
        </span>
        {info ? <InfoTip setting={label} text={info} /> : null}
      </span>
      {children}
      {error ? (
        <span className="dialog__error" role="alert">
          {error}
        </span>
      ) : null}
    </div>
  );
}

export function Input({ mono = false, className, ...rest }: InputHTMLAttributes<HTMLInputElement> & { mono?: boolean }) {
  return <input className={["input", mono ? "mono" : "", className ?? ""].filter(Boolean).join(" ")} {...rest} />;
}

/**
 * Password / passphrase input with a show-hide toggle. `label` is the control's accessible name (the toggle lives
 * inside the wrapping `<label>`, so the name must not be derived from the label's content).
 */
export function SecretInput({
  label,
  className,
  ...rest
}: Omit<InputHTMLAttributes<HTMLInputElement>, "type"> & { label: string }) {
  const { t } = useTranslation();
  const [shown, setShown] = useState(false);
  return (
    <span className="secret-input">
      <input
        className={["input", className ?? ""].filter(Boolean).join(" ")}
        {...rest}
        aria-label={label}
        type={shown ? "text" : "password"}
      />
      <button
        type="button"
        className="icon-button secret-input__toggle"
        aria-label={shown ? t("common.hideSecret") : t("common.showSecret")}
        aria-pressed={shown}
        title={shown ? t("common.hideSecret") : t("common.showSecret")}
        onClick={() => setShown((v) => !v)}
      >
        <Icon name={shown ? "eyeOff" : "eye"} size={15} />
      </button>
    </span>
  );
}

export function Select({ className, children, ...rest }: SelectHTMLAttributes<HTMLSelectElement>) {
  return (
    <select className={["input", className ?? ""].filter(Boolean).join(" ")} {...rest}>
      {children}
    </select>
  );
}

export function Checkbox({
  label,
  checked,
  onChange,
  disabled,
  title,
  hint,
  info,
}: {
  label: string;
  checked: boolean;
  onChange: (checked: boolean) => void;
  disabled?: boolean;
  title?: string;
  /** Help text under the checkbox (outside the label, so it is not part of the accessible name). */
  hint?: string;
  /** Longer explanation, shown by a (?) tip next to the label (the label is linked by `htmlFor`). */
  info?: string;
}) {
  const id = useId();
  if (info) {
    return (
      <div className="check-row">
        <div className="check check--tip" title={title}>
          <label htmlFor={id}>{label}</label>
          <InfoTip setting={label} text={info} />
          <input id={id} type="checkbox" checked={checked} disabled={disabled} onChange={(e) => onChange(e.target.checked)} />
        </div>
        {hint ? <span className="field__hint">{hint}</span> : null}
      </div>
    );
  }
  const box = (
    <label className="check" title={title}>
      <input type="checkbox" checked={checked} disabled={disabled} onChange={(e) => onChange(e.target.checked)} />
      <span>{label}</span>
    </label>
  );
  if (!hint) return box;
  return (
    <div className="check-row">
      {box}
      <span className="field__hint">{hint}</span>
    </div>
  );
}

export interface SegmentOption<V extends string> {
  value: V;
  label: string;
}

/** Radio-style segmented control (Connect.dc authentication switch). */
export function SegmentedControl<V extends string>({
  label,
  options,
  value,
  onChange,
}: {
  label: string;
  options: readonly SegmentOption<V>[];
  value: V;
  onChange: (value: V) => void;
}) {
  const id = useId();
  return (
    <div className="field" role="radiogroup" aria-labelledby={id}>
      <span id={id} className="field__label">
        {label}
      </span>
      <div className="segmented">
        {options.map((o) => (
          <button
            key={o.value}
            type="button"
            role="radio"
            aria-checked={o.value === value}
            className={`segmented__item${o.value === value ? " segmented__item--active" : ""}`}
            onClick={() => onChange(o.value)}
          >
            {o.label}
          </button>
        ))}
      </div>
    </div>
  );
}
