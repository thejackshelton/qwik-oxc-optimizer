
import { component$, useSignal } from '@qwik.dev/core';

export default component$(() => {
  const arr = useSignal(['a', 'b'])
  return (
    <div>
      {arr.value.map((val, i) => {
        const index = i+1;
		const value = val.toUpperCase();
        return <div onClick$={() => console.log(value, index)}>{val}</div>
      })}
    </div>
  );
});
