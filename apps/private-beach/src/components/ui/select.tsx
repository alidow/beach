import * as React from 'react';

type Option = { value: string; label: string };
type SelectProps = {
  value: string;
  onChange: (v: string) => void;
  options: Option[];
  className?: string;
};

export function Select({ value, onChange, options, className = '' }: SelectProps) {
  return (
    <select
      className={`h-9 rounded-md border border-neutral-300 bg-white px-2 text-sm focus:outline-none focus:ring-2 focus:ring-black focus:ring-offset-2 ${className}`}
      value={value}
      onChange={(e) => onChange(e.target.value)}
    >
      {options.map((o) => (
        <option key={o.value} value={o.value}>
          {o.label}
        </option>
      ))}
    </select>
  );
}

