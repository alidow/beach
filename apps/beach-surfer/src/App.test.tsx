import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import App from './App';

describe('App', () => {
  it('renders the connect button', () => {
    render(<App />);
    expect(screen.getByRole('button', { name: /connect/i })).toBeInTheDocument();
  });
});
