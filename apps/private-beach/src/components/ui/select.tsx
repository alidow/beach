import * as React from 'react';

type Option = { value: string; label: string };
type SelectProps = {
  value: string;
  onChange: (v: string) => void;
  options: Option[];
  className?: string;
  id?: string;
  disabled?: boolean;
};

export function Select({ value, onChange, options, className = '', id, disabled }: SelectProps) {
  return (
    <select
      className={`h-9 rounded-md border border-input bg-background px-2 text-sm focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 ring-offset-background ${className}`}
      value={value}
      onChange={(e) => onChange(e.target.value)}
      id={id}
      disabled={disabled}
    >
      {options.map((o) => (
        <option key={o.value} value={o.value}>
          {o.label}
        </option>
      ))}
    </select>
  );
}
