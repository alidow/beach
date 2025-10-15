import AppLegacy from './AppLegacy';
import AppV2 from './AppV2';
import { shouldUseAppV2 } from './lib/featureFlags';

export default function App(): JSX.Element {
  return shouldUseAppV2() ? <AppV2 /> : <AppLegacy />;
}

export { AppLegacy, AppV2 };
