// Render a file with WebKit: webkit_render INPUT.svg OUTPUT.png WIDTH HEIGHT (absolute paths).
import AppKit
import WebKit
let a=CommandLine.arguments
final class D:NSObject,WKNavigationDelegate{
  func webView(_ w:WKWebView,didFinish n:WKNavigation!){
    DispatchQueue.main.asyncAfter(deadline:.now()+0.5){
      w.takeSnapshot(with:nil){img,e in
        guard let img else{print("fail",e as Any);exit(1)}
        let r=NSBitmapImageRep(data:img.tiffRepresentation!)!
        try! r.representation(using:.png,properties:[:])!.write(to:URL(fileURLWithPath:a[2])); exit(0)}}}
}
let app=NSApplication.shared
let w=WKWebView(frame:NSRect(x:0,y:0,width:Double(a[3])!,height:Double(a[4])!))
let win=NSWindow(contentRect:w.frame,styleMask:[.borderless],backing:.buffered,defer:false); win.contentView=w
let d=D(); w.navigationDelegate=d
let u=URL(fileURLWithPath:a[1]); w.loadFileURL(u,allowingReadAccessTo:u.deletingLastPathComponent())
app.run()
