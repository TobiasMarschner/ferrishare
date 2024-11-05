/** @type {import('tailwindcss').Config} */
module.exports = {
  content: ["./templates/**/*.html"],
  theme: {
    extend: {},
  },
  plugins: [],
  safelist: [
    'bg-red-100',
    'text-red-800',
    'bg-green-100',
    'text-green-800',
    'bg-blue-100',
    'text-blue-800',
    'animate-spin',
  ],
}

