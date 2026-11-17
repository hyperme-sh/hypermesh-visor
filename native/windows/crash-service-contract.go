//go:build windows

// A disposable CI service for the Rust client's SCM and named-pipe contract.
package main

import (
 "io"
 "os"
 "time"
 "github.com/Microsoft/go-winio"
 "golang.org/x/sys/windows"
 "golang.org/x/sys/windows/svc"
)
const serviceName="HypermeshCrashRegistration"
type fixture struct{sid string}
func(f fixture) Execute(_ []string,requests <-chan svc.ChangeRequest,changes chan<- svc.Status)(bool,uint32){
 user,err:=windows.GetCurrentProcessToken().GetTokenUser();if err!=nil||user.User.Sid.String()!="S-1-5-18"{return false,1}
 listener,err:=winio.ListenPipe(`\\.\pipe\hypermesh-crash-registration-v1`,&winio.PipeConfig{SecurityDescriptor:"O:SYG:SYD:P(A;;FA;;;SY)(A;;0x12019b;;;"+f.sid+")"});if err!=nil{return false,1};defer listener.Close()
 go func(){for{c,err:=listener.Accept();if err!=nil{return};c.SetDeadline(time.Now().Add(5*time.Second));var raw [13]byte;if _,err:=io.ReadFull(c,raw[:]);err==nil&&string(raw[:])=="unit metadata"{c.Write([]byte("unit accepted"))};c.Close()}}()
 changes<-svc.Status{State:svc.Running,Accepts:svc.AcceptStop|svc.AcceptShutdown}
 for request:=range requests{switch request.Cmd{case svc.Interrogate:changes<-request.CurrentStatus;case svc.Stop,svc.Shutdown:return false,0}}
 return false,0
}
func main(){if len(os.Args)!=2{os.Exit(1)};if _,err:=windows.StringToSid(os.Args[1]);err!=nil{os.Exit(1)};if err:=svc.Run(serviceName,fixture{os.Args[1]});err!=nil{os.Exit(1)}}
