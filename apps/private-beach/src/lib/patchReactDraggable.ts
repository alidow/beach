/* eslint-disable react/no-find-dom-node */
import { DraggableCore } from 'react-draggable';
import { findDOMNode } from 'react-dom';

type DraggableCoreInstance = {
  props?: {
    nodeRef?: { current: Element | null } | null;
  };
};

const proto = (DraggableCore as unknown as { prototype: DraggableCoreInstance & { componentDidMount?: (...args: unknown[]) => unknown } }).prototype;

const originalComponentDidMount = proto.componentDidMount;

proto.componentDidMount = function patchedComponentDidMount(this: DraggableCoreInstance & { componentDidMount?: (...args: unknown[]) => unknown }, ...args: unknown[]) {
  if (typeof originalComponentDidMount === 'function') {
    originalComponentDidMount.apply(this, args);
  }

  const nodeRef = this.props?.nodeRef;
  if (nodeRef && nodeRef.current == null) {
    const resolved = findDOMNode(this as unknown as any);
    if (resolved instanceof Element) {
      nodeRef.current = resolved;
    }
  }
};
