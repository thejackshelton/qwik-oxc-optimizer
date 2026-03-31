
import { component$, useSignal } from '@qwik.dev/core';

export default component$(() => {
  const arr = useSignal(['a', 'b']);
  const items = [];
  for (const val of arr.value) {
    items.push(
      <div onClick$={() => console.log(val)}>{val}</div>
    );
  }
  return <div>{items}</div>;
});
