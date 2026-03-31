
import { component$, useSignal } from '@qwik.dev/core';

export default component$(() => {
  const matrix = useSignal([['a', 'b'], ['c', 'd']]);
  return (
    <div>
      {matrix.value.map((row, i) => (
        <div key={i}>
          {row.map((cell, j) => {
            const cellKey = cell + '-' + j;
            return (
              <span key={j}>
                <button onClick$={() => console.log(cellKey)}>{cell}</button>
              </span>
            );
          })}
        </div>
      ))}
    </div>
  );
});
