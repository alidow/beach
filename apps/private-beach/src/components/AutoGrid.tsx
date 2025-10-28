import { useEffect } from 'react';
import type { ComponentProps } from 'react';
import GridLayout, { WidthProvider } from 'react-grid-layout';

const WidthAwareGrid = WidthProvider(GridLayout);

type Props = ComponentProps<typeof WidthAwareGrid>;

export default function AutoGrid(props: Props) {
  const { innerRef, ...rest } = props;
  useEffect(() => {
    if (typeof window === 'undefined') return;
    console.info('[tile-layout] instrumentation', { component: 'AutoGrid', version: 'v1' });
  }, []);
  if (typeof window !== 'undefined') {
    const summary = {
      layout: rest.layout?.map(({ i, x, y, w, h, minW, maxW, minH, maxH }) => ({
        i,
        x,
        y,
        w,
        h,
        minW,
        maxW,
        minH,
        maxH,
      })),
      cols: rest.cols,
      width: rest.width,
    };
    console.info('[tile-layout] AutoGrid props', JSON.stringify(summary));
  }

  return <WidthAwareGrid innerRef={innerRef} {...rest} />;
}
