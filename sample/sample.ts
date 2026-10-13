// Sample TypeScript: typed models plus a small pure function.

export interface Item {
  sku: string;
  qty: number;
  price: number;
}

export interface Order {
  id: string;
  currency: "USD" | "EUR";
  items: Item[];
}

export function total(order: Order): number {
  const sum = order.items.reduce((acc, item) => acc + item.qty * item.price, 0);
  return Math.round(sum * 100) / 100;
}

const example: Order = {
  id: "order-1",
  currency: "USD",
  items: [
    { sku: "A-100", qty: 2, price: 9.99 },
    { sku: "B-200", qty: 1, price: 19.5 },
  ],
};

console.log(`total(${example.id}) = ${total(example)} ${example.currency}`);
