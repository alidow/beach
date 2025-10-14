import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import AppV2 from './AppV2';

describe('App', () => {
  it('renders the connect button', () => {
    render(<AppV2 />);
    expect(screen.getByRole('button', { name: /connect/i })).toBeInTheDocument();
  });
});
