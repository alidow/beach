import * as React from 'react';

type ButtonProps = React.ButtonHTMLAttributes<HTMLButtonElement> & {
  variant?: 'primary' | 'ghost' | 'danger' | 'outline';
  size?: 'sm' | 'md';
};

export function Button({ className = '', variant = 'primary', size = 'md', ...props }: ButtonProps) {
  const base =
    'inline-flex items-center justify-center rounded-md font-medium transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 ring-offset-background disabled:opacity-50 disabled:pointer-events-none';
  const sizes = size === 'sm' ? 'h-8 px-3 text-sm' : 'h-9 px-4 text-sm';
  let colors = '';
  switch (variant) {
    case 'primary':
      colors = 'bg-primary text-primary-foreground hover:bg-primary/90';
      break;
    case 'ghost':
      colors = 'bg-transparent text-foreground hover:bg-accent hover:text-accent-foreground';
      break;
    case 'outline':
      colors = 'border border-input bg-background text-foreground hover:bg-accent hover:text-accent-foreground';
      break;
    case 'danger':
      colors = 'bg-destructive text-destructive-foreground hover:bg-destructive/90';
      break;
  }
  return <button className={`${base} ${sizes} ${colors} ${className}`} {...props} />;
}
