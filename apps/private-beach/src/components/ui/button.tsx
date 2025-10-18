import * as React from 'react';

type ButtonProps = React.ButtonHTMLAttributes<HTMLButtonElement> & {
  variant?: 'primary' | 'ghost' | 'danger' | 'outline';
  size?: 'sm' | 'md';
};

export function Button({ className = '', variant = 'primary', size = 'md', ...props }: ButtonProps) {
  const base = 'inline-flex items-center justify-center rounded-md font-medium transition-colors focus:outline-none focus:ring-2 focus:ring-offset-2 disabled:opacity-50 disabled:pointer-events-none';
  const sizes = size === 'sm' ? 'h-8 px-3 text-sm' : 'h-9 px-4 text-sm';
  let colors = '';
  switch (variant) {
    case 'primary':
      colors = 'bg-black text-white hover:bg-neutral-800 focus:ring-black ring-offset-white';
      break;
    case 'ghost':
      colors = 'bg-transparent hover:bg-neutral-100 text-neutral-900 ring-offset-white';
      break;
    case 'outline':
      colors = 'border border-neutral-300 hover:bg-neutral-100 text-neutral-900 ring-offset-white';
      break;
    case 'danger':
      colors = 'bg-red-600 text-white hover:bg-red-700 focus:ring-red-600 ring-offset-white';
      break;
  }
  return <button className={`${base} ${sizes} ${colors} ${className}`} {...props} />;
}

