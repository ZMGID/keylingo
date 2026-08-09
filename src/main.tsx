import React from 'react'
import ReactDOM from 'react-dom/client'
import App from './App.tsx'
import './index.css'
import 'streamdown/styles.css'

// 屏蔽 WebView 原生右键菜单（Back/Reload/Inspect）
document.addEventListener('contextmenu', (e) => e.preventDefault())

// React 应用入口文件
// 使用 createRoot 将 App 组件挂载到 DOM 的 #root 元素上
ReactDOM.createRoot(document.getElementById('root')!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
)
